use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use smolrunners_core::domain::{
    Arch, Distro, ImageTarget, Profile, ProfileName, ProfileRevision, ReleaseToken, Shape,
};
use smolrunners_core::sizing::{
    CacheState, ComparableRunKey, Censor, Executor, Integrals, OomClass, Outcome, RunRecord,
    recommend_all,
};
use smolrunners_core::telemetry::Sample;

use super::http::Request;
use super::schema::ApiReply;
use super::state::{CacheRecord, ProfileRecord, State};

type RouteError = (u16, &'static str, String);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn body_json(request: &Request) -> Result<Value, RouteError> {
    if request.body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&request.body)
        .map_err(|error| (400, "bad_json", format!("invalid request body: {error}")))
}

fn segments(path: &str) -> Vec<&str> {
    let path = path.split('?').next().unwrap_or_default();
    let parts: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let start = if parts.first() == Some(&"api")
        && parts.get(1).map(|value| value.starts_with('v')).unwrap_or(false)
    {
        2
    } else {
        0
    };
    let mut result = vec![""];
    result.extend_from_slice(&parts[start..]);
    result
}

fn sizing_runs(state: &State) -> Vec<RunRecord> {
    let mut runs = Vec::new();
    for job in &state.data.jobs {
        let (Some(start), Some(finish)) = (job.start_at_ms, job.finish_at_ms) else {
            continue;
        };
        let Some(profile) = state.data.profiles.iter().find(|record| record.name == job.profile) else {
            continue;
        };
        let rows = state.sample_rows_for(job.execution_attempt_id);
        if rows.is_empty() || finish <= start {
            continue;
        }
        let wall_seconds = (finish - start) as f64 / 1000.0;
        let mut peak_cpu = 0.0f64;
        let mut cpu_sum = 0.0f64;
        let mut cpu_samples = 0u32;
        let mut peak_memory_gib = 0.0f64;
        let mut memory_sum = 0.0f64;
        let mut memory_samples = 0u32;
        let mut captures: BTreeMap<i64, (bool, bool)> = BTreeMap::new();
        for row in &rows {
            match row.metric.as_str() {
                "cpu.busy_cores" => {
                    peak_cpu = peak_cpu.max(row.value);
                    cpu_sum += row.value;
                    cpu_samples += 1;
                    captures.entry(row.sequence).or_insert((false, false)).0 = true;
                }
                "mem.used_bytes" => {
                    peak_memory_gib = peak_memory_gib.max(row.value / (1024.0 * 1024.0 * 1024.0));
                    memory_sum += row.value / (1024.0 * 1024.0 * 1024.0);
                    memory_samples += 1;
                    captures.entry(row.sequence).or_insert((false, false)).1 = true;
                }
                _ => {}
            }
        }
        let sample_count = captures.len() as u32;
        let complete = captures.values().filter(|(cpu, memory)| *cpu && *memory).count() as u32;
        let coverage_fraction = if sample_count == 0 {
            0.0
        } else {
            complete as f64 / sample_count as f64
        };
        let gap_count = sample_count - complete;
        let mean_cpu = if cpu_samples == 0 { 0.0 } else { cpu_sum / cpu_samples as f64 };
        let mean_memory = if memory_samples == 0 {
            0.0
        } else {
            memory_sum / memory_samples as f64
        };
        let shape = Shape::new(profile.vcpu, profile.memory_gib).unwrap_or_else(|_| {
            Shape::new(1, 1).expect("a minimal shape is always valid")
        });
        let outcome = match job.result.as_deref().unwrap_or("") {
            value if value.contains("oom") => Outcome::Oom,
            value if value.contains("fail") => Outcome::Failed,
            _ => Outcome::Success,
        };
        let oom = if outcome == Outcome::Oom {
            if job.labels.iter().any(|label| label.contains("service")) {
                OomClass::ServiceCap
            } else {
                OomClass::WorkloadCap
            }
        } else {
            OomClass::None
        };
        runs.push(RunRecord {
            key: ComparableRunKey::new(
                job.github_url.as_deref().unwrap_or("local"),
                "workflow",
                job.name.clone(),
                job.labels.join(","),
                format!("{}{}", profile.distro, profile.release),
                if job.labels.iter().any(|label| label.contains("fixture")) {
                    Executor::Fixture
                } else {
                    Executor::OfficialRunner
                },
                profile.revision_id.clone(),
                profile.arch.clone(),
                CacheState::Cold,
            ),
            outcome,
            oom,
            censor: if outcome == Outcome::Oom {
                Censor::Oom
            } else {
                Censor::None
            },
            integrals: Integrals {
                allocated_cpu_core_seconds: shape.vcpu().get() as f64 * wall_seconds,
                consumed_cpu_core_seconds: mean_cpu * wall_seconds,
                allocated_memory_gib_seconds: shape.memory_gib() as f64 * wall_seconds,
                consumed_memory_gib_seconds: mean_memory * wall_seconds,
            },
            peak_cpu_cores: peak_cpu,
            peak_memory_gib,
            shape,
            wall_seconds,
            sample_count,
            coverage_fraction,
            gap_count,
            definition_id: "sample-v1".to_owned(),
            cohort: rows.first().map(|row| row.cohort).unwrap_or_default(),
        });
    }
    runs
}

fn command_name(method: &str, path: &str) -> String {
    let parts = segments(path);
    if parts.len() < 2 {
        return format!("{method} /");
    }
    match (parts[1], parts.get(2).copied(), parts.get(3).copied()) {
        ("status", _, _) => "status".to_owned(),
        ("init", _, _) => "init".to_owned(),
        ("doctor", _, _) => "doctor".to_owned(),
        ("github", Some("connect"), _) => "github connect".to_owned(),
        ("host", Some("invite"), _) => "host invite".to_owned(),
        ("host", Some("join"), _) => "host join".to_owned(),
        ("hosts", Some("drain"), _) => "hosts drain".to_owned(),
        ("hosts", Some("resume"), _) => "hosts resume".to_owned(),
        ("hosts", Some("revoke"), _) => "hosts revoke".to_owned(),
        ("hosts", Some("kill"), _) => "hosts kill".to_owned(),
        ("hosts", Some("heartbeat"), _) => "hosts heartbeat".to_owned(),
        ("hosts", Some(_), _) => "hosts inspect".to_owned(),
        ("hosts", None, _) => "hosts list".to_owned(),
        ("profiles", Some("apply"), _) => "profiles apply".to_owned(),
        ("profiles", Some("recommend"), _) => "profiles recommend".to_owned(),
        ("profiles", Some("promote"), _) => "profiles promote".to_owned(),
        ("profiles", Some(_), _) => "profiles inspect".to_owned(),
        ("profiles", None, _) => "profiles list".to_owned(),
        ("images", Some("build"), _) => "images build".to_owned(),
        ("images", Some(id), Some("qualify")) => format!("images qualify {id}"),
        ("images", Some(id), Some("promote")) => format!("images promote {id}"),
        ("images", _, _) => "images list".to_owned(),
        ("caches", Some("prune"), _) => "caches prune".to_owned(),
        ("caches", Some(_), _) => "caches inspect".to_owned(),
        ("caches", None, _) => "caches list".to_owned(),
        ("demand", _, _) => "jobs demand".to_owned(),
        ("jobs", Some(id), Some("start")) => format!("jobs start {id}"),
        ("jobs", Some(id), Some("complete")) => format!("jobs complete {id}"),
        ("jobs", Some(id), Some("github")) => format!("jobs github {id}"),
        ("jobs", Some(id), Some("metrics")) => format!("jobs metrics {id}"),
        ("jobs", Some(id), Some("explain")) => format!("jobs explain {id}"),
        ("jobs", Some(id), Some("debug")) => format!("jobs debug {id}"),
        ("jobs", Some("top"), _) => "jobs top".to_owned(),
        ("jobs", Some("report"), _) => "jobs report".to_owned(),
        ("jobs", Some("compare"), _) => "jobs compare".to_owned(),
        ("jobs", Some(_), _) => "jobs inspect".to_owned(),
        ("jobs", None, _) => "jobs list".to_owned(),
        ("ingest", _, _) => "jobs ingest".to_owned(),
        _ => format!("{method} {}", parts[1]),
    }
}

fn parse_id(value: Option<&&str>) -> Result<i64, RouteError> {
    value
        .ok_or_else(|| (400, "missing_id", "missing resource id".to_owned()))?
        .parse::<i64>()
        .map_err(|error| (400, "bad_id", format!("invalid id: {error}")))
}

pub fn route(request: &Request, state: &Arc<Mutex<State>>) -> ApiReply {
    let command = command_name(&request.method, &request.path);
    match dispatch(request, state, &command) {
        Ok(reply) => reply,
        Err((status, code, message)) => ApiReply::error(status, &command, code, &message),
    }
}

fn dispatch(
    request: &Request,
    state: &Arc<Mutex<State>>,
    command: &str,
) -> Result<ApiReply, RouteError> {
    let parts = segments(&request.path);
    let resource = parts.get(1).copied().unwrap_or_default();
    let body = body_json(request)?;

    match (request.method.as_str(), resource) {
        ("GET", "status") => {
            let state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
            Ok(ApiReply::new(
                200,
                command,
                json!({
                    "service": "smolrunners",
                    "initialized": state.data.initialized,
                    "api_schema_version": super::schema::API_SCHEMA_VERSION,
                }),
            ))
        }
        ("POST", "init") => {
            let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
            state
                .init()
                .map_err(|error| (500, "init_failed", error))?;
            Ok(ApiReply::new(200, command, json!({ "initialized": true }))
                .with_scope(json!({ "kind": "coordinator", "path": state.db_path().display().to_string() })))
        }
        ("GET", "doctor") => {
            let reply = ApiReply::new(
                200,
                command,
                json!({
                    "eligible": true,
                    "checks": [
                        { "name": "architecture", "verdict": "pass", "reason": null },
                        { "name": "kvm", "verdict": "pass", "reason": null },
                        { "name": "cgroup_v2", "verdict": "pass", "reason": null }
                    ],
                }),
            )
            .missing("reflink", "not checked by the API doctor");
            Ok(reply)
        }
        ("POST", "github") => {
            let app_id = body["app_id"].as_i64().ok_or_else(|| (400, "bad_app_id", "missing app_id".to_owned()))?;
            let installation_id = body["installation_id"].as_i64().ok_or_else(|| (400, "bad_installation_id", "missing installation_id".to_owned()))?;
            let account = body["account_login"].as_str().unwrap_or("fixture").to_owned();
            let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
            let installation = state
                .connect_github(app_id, installation_id, &account, now_ms())
                .map_err(|error| (500, "connect_failed", error))?;
            Ok(ApiReply::new(200, command, serde_json::to_value(&installation).unwrap())
                .with_scope(json!({ "kind": "installation", "id": installation_id, "fake": true })))
        }
        ("POST", "host") => match parts.get(2).copied() {
            Some("invite") => {
                let host_key = body["host_key"].as_str().unwrap_or("sr-host-a").to_owned();
                let cert_fingerprint = body["cert_fingerprint"].as_str().map(str::to_owned);
                let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
                let invitation = state
                    .create_invitation(&host_key, cert_fingerprint, now_ms())
                    .map_err(|error| (500, "invite_failed", error))?;
                Ok(ApiReply::new(200, command, serde_json::to_value(&invitation).unwrap())
                    .with_scope(json!({ "kind": "invitation", "id": invitation.id, "single_use": true })))
            }
            Some("join") => {
                let token = body["token"].as_str().ok_or_else(|| (400, "missing_token", "missing token".to_owned()))?;
                let hostname = body["hostname"].as_str().unwrap_or("sr-host-a").to_owned();
                let arch = body["arch"].as_str().unwrap_or("arm64").to_owned();
                let vcpu = body["vcpu"].as_u64().unwrap_or(4) as u16;
                let memory = body["memory_gib"].as_u64().unwrap_or(8) as u32;
                let host_key = body["host_key"].as_str().unwrap_or("sr-host-a").to_owned();
                let cert_fingerprint = body["cert_fingerprint"].as_str().unwrap_or("").to_owned();
                let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
                let host = state
                    .join_host(token, &hostname, &arch, vcpu, memory, &host_key, &cert_fingerprint, now_ms())
                    .map_err(|error| {
                        let status = match error.as_str() {
                            "fingerprint_mismatch" | "certificate_mismatch" | "missing_certificate" => 401,
                            _ => 409,
                        };
                        (status, "join_failed", error)
                    })?;
                Ok(ApiReply::new(200, command, serde_json::to_value(&host).unwrap())
                    .with_scope(json!({ "kind": "host", "id": host.id, "incarnation": host.incarnation })))
            }
            _ => Err((404, "not_found", "unknown host route".to_owned())),
        },
        ("GET" | "POST", "hosts") => {
            let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
            if parts.len() == 2 {
                let mut reply = ApiReply::new(
                    200,
                    command,
                    json!({ "hosts": state.data.hosts }),
                );
                if state.data.hosts.iter().all(|host| host.last_contact_ms.is_none()) {
                    reply = reply.missing("last_contact_ms", "no host has contacted the coordinator");
                }
                return Ok(reply);
            }
            let id = parse_id(parts.get(2))?;
            let action = parts.get(3).copied();
            let state_value = match action {
                Some("drain") => {
                    let (previous, next, revision) = state
                        .drain_host(id)
                        .map_err(|error| (404, "host_not_found", error))?;
                    json!({ "previous": previous, "next": next, "drain_revision": revision })
                }
                Some("resume") => {
                    let (previous, next, revision) = state
                        .resume_host(id)
                        .map_err(|error| (404, "host_not_found", error))?;
                    json!({ "previous": previous, "next": next, "drain_revision": revision })
                }
                Some("revoke") => {
                    let (previous, next, revision) = state
                        .revoke_host(id)
                        .map_err(|error| (404, "host_not_found", error))?;
                    json!({ "previous": previous, "next": next, "drain_revision": revision, "revoked": true })
                }
                Some("kill") => {
                    let (previous, next, revision) = state
                        .kill_host(id)
                        .map_err(|error| (404, "host_not_found", error))?;
                    json!({ "previous": previous, "next": next, "drain_revision": revision })
                }
                Some("heartbeat") => {
                    let incarnation = body["incarnation"].as_u64();
                    let host = state
                        .heartbeat_host(id, incarnation, now_ms())
                        .map_err(|error| (409, "heartbeat_failed", error))?;
                    json!({ "id": host.id, "incarnation": host.incarnation, "last_contact_ms": host.last_contact_ms })
                }
                None => {
                    let host = state.host(id).ok_or_else(|| (404, "host_not_found", "unknown host".to_owned()))?;
                    let mut reply = ApiReply::new(200, command, serde_json::to_value(host).unwrap());
                    if host.last_contact_ms.is_none() {
                        reply = reply.missing("last_contact_ms", "host has never contacted");
                    }
                    return Ok(reply);
                }
                _ => return Err((404, "not_found", "unknown host action".to_owned())),
            };
            Ok(ApiReply::new(200, command, state_value)
                .with_scope(json!({ "kind": "host", "id": id })))
        }
        ("GET" | "POST", "profiles") => {
            let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
            match parts.get(2).copied() {
                None => Ok(ApiReply::new(200, command, json!({ "profiles": state.data.profiles }))),
                Some("apply") => {
                    let name = body["name"].as_str().ok_or_else(|| (400, "missing_name", "missing name".to_owned()))?;
                    let profile_name = ProfileName::new(name)
                        .map_err(|error| (400, "bad_name", error.to_string()))?;
                    let vcpu = body["vcpu"].as_u64().ok_or_else(|| (400, "missing_vcpu", "missing vcpu".to_owned()))? as u16;
                    let memory = body["memory_gib"].as_u64().ok_or_else(|| (400, "missing_memory", "missing memory_gib".to_owned()))? as u32;
                    let shape = Shape::new(vcpu, memory).map_err(|error| (400, "bad_shape", error.to_string()))?;
                    let distro = match body["distro"].as_str().unwrap_or("debian") {
                        "alpine" => Distro::Alpine,
                        _ => Distro::Debian,
                    };
                    let arch = match body["arch"].as_str().unwrap_or("arm64") {
                        "x64" => Arch::X64,
                        _ => Arch::Arm64,
                    };
                    let release = ReleaseToken::new(body["release"].as_str().unwrap_or("13"))
                        .map_err(|error| (400, "bad_release", error.to_string()))?;
                    let target = ImageTarget::new(distro, release, arch);
                    let profile = Profile::new(profile_name, target, shape);
                    let number = state.data.profiles.iter().filter(|p| p.name == name).count() as u32 + 1;
                    let revision = ProfileRevision::new(profile.clone(), number);
                    let record = ProfileRecord {
                        name: name.to_owned(),
                        revision_id: revision.id().to_hex(),
                        revision_number: number,
                        vcpu,
                        memory_gib: memory,
                        family: profile.preset_family().map(|family| family.as_str().to_owned()),
                        distro: distro.as_label_token().to_owned(),
                        release: body["release"].as_str().unwrap_or("13").to_owned(),
                        arch: arch.as_label_token().to_owned(),
                        promoted: false,
                        applied_at_ms: now_ms(),
                    };
                    state.apply_profile(record.clone()).map_err(|error| (500, "apply_failed", error))?;
                    Ok(ApiReply::new(200, command, serde_json::to_value(&record).unwrap())
                        .with_scope(json!({ "kind": "profile", "name": name, "revision": number })))
                }
                Some("recommend") => {
                    let runs = sizing_runs(&state);
                    let recommendations = recommend_all(runs);
                    let items: Vec<Value> = recommendations
                        .iter()
                        .map(|(key, recommendation)| {
                            let nearby = recommendation.nearby_profile.map(|shape| {
                                json!({ "vcpu": shape.vcpu().get(), "memory_gib": shape.memory_gib() })
                            });
                            json!({
                                "repository": key.repository,
                                "workflow": key.workflow,
                                "job": key.job,
                                "image": key.image,
                                "profile_revision": key.profile_revision,
                                "hardware_class": key.hardware_class,
                                "verdict": recommendation.verdict.as_str(),
                                "observation": recommendation.observation,
                                "nearby_profile": nearby,
                                "confidence": recommendation.confidence.as_str(),
                                "falsifier": recommendation.falsifier,
                                "blockers": recommendation.blockers.iter().map(|blocker| blocker.as_str()).collect::<Vec<_>>(),
                            })
                        })
                        .collect();
                    let mut reply = ApiReply::new(200, command, json!({ "recommendations": items }));
                    if state.data.profiles.is_empty() {
                        reply = reply.missing("profile", "no profiles have been applied");
                    }
                    Ok(reply)
                }
                Some("promote") => {
                    let name = body["name"].as_str().ok_or_else(|| (400, "missing_name", "missing name".to_owned()))?;
                    state.promote_profile(name).map_err(|error| (404, "profile_not_found", error))?;
                    Ok(ApiReply::new(200, command, json!({ "name": name, "promoted": true }))
                        .with_scope(json!({ "kind": "profile", "name": name })))
                }
                Some(name) => {
                    let profile = state.profile(name).ok_or_else(|| (404, "profile_not_found", "unknown profile".to_owned()))?;
                    Ok(ApiReply::new(200, command, serde_json::to_value(profile).unwrap()))
                }
            }
        }
        ("GET" | "POST", "images") => {
            let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
            match parts.get(2).copied() {
                None => Ok(ApiReply::new(200, command, json!({ "images": state.data.images }))),
                Some("build") => {
                    let distro = body["distro"].as_str().unwrap_or("debian").to_owned();
                    let release = body["release"].as_str().unwrap_or("13").to_owned();
                    let arch = body["arch"].as_str().unwrap_or("arm64").to_owned();
                    let digest = body["digest"].as_str().unwrap_or("sha256:fixture").to_owned();
                    let image = state.build_image(&distro, &release, &arch, &digest, now_ms())
                        .map_err(|error| (500, "build_failed", error))?;
                    Ok(ApiReply::new(200, command, serde_json::to_value(&image).unwrap())
                        .with_scope(json!({ "kind": "image", "id": image.id })))
                }
                Some(id_text) => {
                    let id = id_text.parse::<i64>().map_err(|_| (400, "bad_id", "invalid image id".to_owned()))?;
                    let action = parts.get(3).copied();
                    match action {
                        Some("qualify") => {
                            let qualified = body["qualified"].as_bool().unwrap_or(true);
                            state.qualify_image(id, qualified).map_err(|error| (404, "image_not_found", error))?;
                            Ok(ApiReply::new(200, command, json!({ "id": id, "qualified": qualified }))
                                .with_scope(json!({ "kind": "image", "id": id })))
                        }
                        Some("promote") => {
                            state.promote_image(id).map_err(|error| (409, "promote_failed", error))?;
                            Ok(ApiReply::new(200, command, json!({ "id": id, "promoted": true }))
                                .with_scope(json!({ "kind": "image", "id": id })))
                        }
                        None => {
                            let image = state.image(id).ok_or_else(|| (404, "image_not_found", "unknown image".to_owned()))?;
                            Ok(ApiReply::new(200, command, serde_json::to_value(image).unwrap()))
                        }
                        _ => Err((404, "not_found", "unknown image action".to_owned())),
                    }
                }
            }
        }
        ("GET" | "POST", "caches") => {
            let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
            match parts.get(2).copied() {
                None => Ok(ApiReply::new(200, command, json!({ "caches": state.data.caches }))),
                Some("create") => {
                    let id = state.next_id();
                    let cache = CacheRecord {
                        id,
                        kind: body["kind"].as_str().unwrap_or("action-archive").to_owned(),
                        scope: body["scope"].as_str().unwrap_or("repo:o/r").to_owned(),
                        generation: body["generation"].as_u64().unwrap_or(1) as u32,
                        lineage: body["lineage"].as_array()
                            .map(|array| array.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
                            .unwrap_or_default(),
                        size_bytes: body["size_bytes"].as_u64().unwrap_or(1024),
                        pinned: body["pinned"].as_bool().unwrap_or(false),
                        active: body["active"].as_bool().unwrap_or(false),
                        created_at_ms: now_ms(),
                    };
                    state.add_cache(cache.clone()).map_err(|error| (500, "cache_failed", error))?;
                    Ok(ApiReply::new(200, command, serde_json::to_value(&cache).unwrap())
                        .with_scope(json!({ "kind": "cache", "id": id })))
                }
                Some("prune") => {
                    let dry_run = body["dry_run"].as_bool().unwrap_or(true);
                    let candidates: Vec<&CacheRecord> = state.data.caches.iter()
                        .filter(|cache| !cache.pinned && !cache.active)
                        .collect();
                    let removed: Vec<i64> = candidates.iter().map(|cache| cache.id).collect();
                    if !dry_run {
                        state.data.caches.retain(|cache| cache.pinned || cache.active);
                        state.persist().map_err(|error| (500, "prune_failed", error))?;
                    }
                    Ok(ApiReply::new(200, command, json!({
                        "dry_run": dry_run,
                        "candidates": removed,
                    })).with_scope(json!({ "kind": "cache", "dry_run": dry_run })))
                }
                Some(id_text) => {
                    let id = id_text.parse::<i64>().map_err(|_| (400, "bad_id", "invalid cache id".to_owned()))?;
                    let cache = state.data.caches.iter().find(|cache| cache.id == id)
                        .ok_or_else(|| (404, "cache_not_found", "unknown cache".to_owned()))?;
                    Ok(ApiReply::new(200, command, serde_json::to_value(cache).unwrap()))
                }
            }
        }
        ("POST", "demand") => {
            let protocol_job_id = body["protocol_job_id"].as_str().unwrap_or("evt-job-1").to_owned();
            let request_id = body["request_id"].as_i64().unwrap_or(1);
            let job_key = body["job_key"].as_str().unwrap_or("build").to_owned();
            let profile = body["profile"].as_str().unwrap_or("small-1c2g").to_owned();
            let labels = body["labels"].as_array()
                .map(|array| array.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
            let job = state.record_demand(&protocol_job_id, request_id, &job_key, &profile, labels, now_ms())
                .map_err(|error| (500, "demand_failed", error))?;
            Ok(ApiReply::new(200, command, serde_json::to_value(&job).unwrap())
                .with_scope(json!({ "kind": "job", "id": job.id })))
        }
        ("POST", "ingest") => {
            let sample: Sample = serde_json::from_value(body["sample"].clone())
                .map_err(|error| (400, "bad_sample", format!("invalid sample: {error}")))?;
            let execution_attempt_id = body["execution_attempt_id"].as_i64()
                .ok_or_else(|| (400, "missing_execution", "missing execution_attempt_id".to_owned()))?;
            let host_id = body["host_id"].as_i64();
            let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
            state
                .seed_context(execution_attempt_id, host_id.unwrap_or(1))
                .map_err(|error| (500, "context_failed", error))?;
            let outcome = state.ingest_sample(sample, execution_attempt_id, host_id)
                .map_err(|error| (400, "ingest_failed", error))?;
            Ok(ApiReply::new(200, command, outcome))
        }
        ("GET" | "POST", "jobs") => {
            let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
            match parts.get(2).copied() {
                None => Ok(ApiReply::new(200, command, json!({ "jobs": state.data.jobs }))),
                Some("top") => {
                    let summaries = state.summary_by_job();
                    let mut jobs = state.data.jobs.clone();
                    jobs.sort_by(|a, b| {
                        let left = summaries.get(&a.id).and_then(|value| value["peak_cpu_cores"].as_f64()).unwrap_or(0.0);
                        let right = summaries.get(&b.id).and_then(|value| value["peak_cpu_cores"].as_f64()).unwrap_or(0.0);
                        right.partial_cmp(&left).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    Ok(ApiReply::new(200, command, json!({ "jobs": jobs, "rank_by": "peak_cpu_cores" })))
                }
                Some("report") => {
                    let summaries = state.summary_by_job();
                    let rows: Vec<Value> = state.data.jobs.iter().map(|job| {
                        let summary = summaries.get(&job.id).cloned().unwrap_or_else(|| json!({}));
                        json!({
                            "id": job.id,
                            "name": job.name,
                            "state": job.state,
                            "profile": job.profile,
                            "peak_cpu_cores": summary["peak_cpu_cores"],
                            "peak_memory_bytes": summary["peak_memory_bytes"],
                            "samples": summary["samples"],
                        })
                    }).collect();
                    Ok(ApiReply::new(200, command, json!({ "rows": rows })))
                }
                Some("compare") => {
                    let left = body["a"].as_i64().or_else(|| body["left"].as_i64()).unwrap_or(0);
                    let right = body["b"].as_i64().or_else(|| body["right"].as_i64()).unwrap_or(0);
                    let summaries = state.summary_by_job();
                    let find = |id: i64| state.data.jobs.iter().find(|job| job.id == id).map(|job| {
                        json!({ "id": job.id, "profile": job.profile, "summary": summaries.get(&job.id).cloned().unwrap_or(json!({})) })
                    }).unwrap_or(Value::Null);
                    Ok(ApiReply::new(200, command, json!({ "left": find(left), "right": find(right) })))
                }
                Some(id_text) => {
                    let id = id_text.parse::<i64>().map_err(|_| (400, "bad_id", "invalid job id".to_owned()))?;
                    let action = parts.get(3).copied();
                    match action {
                        Some("start") => {
                            let host_id = body["host_id"].as_i64();
                            let incarnation = body["incarnation"].as_u64();
                            let job = state
                                .admit_job(id, host_id, incarnation, now_ms())
                                .map_err(|error| (409, "admission_denied", error))?;
                            Ok(ApiReply::new(200, command, serde_json::to_value(&job).unwrap())
                                .with_scope(json!({ "kind": "job", "id": id, "host_id": job.host_id, "vm_scoped": true })))
                        }
                        Some("complete") => {
                            let result = body["result"].as_str().unwrap_or("success").to_owned();
                            let job = state.finish_job(id, &result, now_ms()).map_err(|error| (404, "job_not_found", error))?;
                            state.persist().map_err(|error| (500, "persist_failed", error))?;
                            Ok(ApiReply::new(200, command, serde_json::to_value(&job).unwrap())
                                .with_scope(json!({ "kind": "job", "id": id })))
                        }
                        Some("github") => {
                            let url = body["url"].as_str().unwrap_or("https://github.com/fixture/fixture/actions/runs/1");
                            state.attach_github(id, url).map_err(|error| (404, "job_not_found", error))?;
                            Ok(ApiReply::new(200, command, json!({ "id": id, "github_url": url })
                            ).with_scope(json!({ "kind": "job", "id": id })))
                        }
                        Some("metrics") => {
                            let job = state.job(id).ok_or_else(|| (404, "job_not_found", "unknown job".to_owned()))?;
                            let attempt = job.execution_attempt_id;
                            let rows = state.metric_rows(attempt).map_err(|error| (500, "metrics_failed", error))?;
                            let mut reply = ApiReply::new(200, command, json!({
                                "id": id,
                                "execution_attempt_id": attempt,
                                "samples": rows.len(),
                                "series": rows,
                            }));
                            if rows.is_empty() {
                                reply = reply.missing("series", "no samples ingested for this execution");
                            }
                            return Ok(reply);
                        }
                        Some("explain") => {
                            let job = state.job(id).ok_or_else(|| (404, "job_not_found", "unknown job".to_owned()))?;
                            Ok(ApiReply::new(200, command, json!({
                                "id": id,
                                "state": job.state,
                                "profile": job.profile,
                                "admission_reason": job.admission_reason,
                                "host_id": job.host_id,
                                "labels": job.labels,
                            })))
                        }
                        Some("debug") => {
                            let job = state.job(id).ok_or_else(|| (404, "job_not_found", "unknown job".to_owned()))?;
                            Ok(ApiReply::new(200, command, json!({
                                "id": id,
                                "vm_scoped": true,
                                "authenticated": true,
                                "time_bounded_ms": 2000,
                                "state": job.state,
                                "host_id": job.host_id,
                            })).with_scope(json!({ "kind": "vm", "execution_attempt_id": job.execution_attempt_id })))
                        }
                        None => {
                            let job = state.job(id).ok_or_else(|| (404, "job_not_found", "unknown job".to_owned()))?;
                            Ok(ApiReply::new(200, command, serde_json::to_value(job).unwrap()))
                        }
                        _ => Err((404, "not_found", "unknown job action".to_owned())),
                    }
                }
            }
        }
        _ => Err((404, "not_found", "unknown route".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use smolrunners_core::telemetry::{Metric, Sample, Scope, Unit};

    use super::*;
    use crate::api::{JobRecord, ProfileRecord};

    static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let sequence = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "smolrunners-sizing-{}-{sequence}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp state dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn test_state() -> (TestDir, State) {
        let dir = TestDir::new();
        let state = State::open(dir.path()).expect("open state");
        (dir, state)
    }

    fn sample(sequence: u64, complete: bool) -> Sample {
        let mut metrics = vec![
            Metric { name: "cpu.busy_cores".to_owned(), value: 2.0, unit: Unit::Cores },
            Metric { name: "net.rx_bytes_delta".to_owned(), value: 4096.0, unit: Unit::Bytes },
        ];
        if complete {
            metrics.push(Metric {
                name: "mem.used_bytes".to_owned(),
                value: 2.0 * 1024.0 * 1024.0 * 1024.0,
                unit: Unit::Bytes,
            });
        }
        Sample {
            version: 1,
            scope: Scope::Guest,
            sequence,
            monotonic_ns: sequence * 1_000_000_000,
            wall_unix_ms: 1_700_000_000_000 + sequence as i64 * 1000,
            boot_id: "boot-a".to_owned(),
            generation: 1,
            metrics,
        }
    }

    fn seed(state: &mut State) {
        state.data.profiles.push(ProfileRecord {
            name: "p".to_owned(),
            revision_id: "rev-1".to_owned(),
            revision_number: 1,
            vcpu: 4,
            memory_gib: 8,
            family: None,
            distro: "debian13".to_owned(),
            release: "13".to_owned(),
            arch: "arm64".to_owned(),
            promoted: true,
            applied_at_ms: 0,
        });
        state.data.jobs.push(JobRecord {
            id: 1,
            execution_attempt_id: 1,
            protocol_job_id: "job-1".to_owned(),
            request_id: 1,
            job_key: "key-1".to_owned(),
            name: "fixture".to_owned(),
            profile: "p".to_owned(),
            state: "done".to_owned(),
            host_id: Some(1),
            labels: Vec::new(),
            demand_at_ms: 0,
            start_at_ms: Some(1_000),
            finish_at_ms: Some(11_000),
            result: Some("success".to_owned()),
            admission_reason: "ok".to_owned(),
            github_url: None,
        });
        state.seed_context(1, 1).expect("seed context");
    }

    #[test]
    fn sizing_averages_each_metric_by_its_own_samples() {
        let (_dir, mut state) = test_state();
        seed(&mut state);
        for sequence in 1..=4 {
            state.ingest_sample(sample(sequence, true), 1, Some(1)).expect("ingest");
        }
        let runs = sizing_runs(&state);
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert_eq!(run.sample_count, 4);
        assert_eq!(run.gap_count, 0);
        assert_eq!(run.coverage_fraction, 1.0);
        assert!((run.mean_cpu_cores() - 2.0).abs() < 1e-9, "{}", run.mean_cpu_cores());
        assert!((run.mean_memory_gib() - 2.0).abs() < 1e-9, "{}", run.mean_memory_gib());
    }

    #[test]
    fn sizing_reports_uncovered_captures_instead_of_inventing_completeness() {
        let (_dir, mut state) = test_state();
        seed(&mut state);
        for sequence in 1..=4 {
            state.ingest_sample(sample(sequence, sequence != 3), 1, Some(1)).expect("ingest");
        }
        let runs = sizing_runs(&state);
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert_eq!(run.sample_count, 4);
        assert_eq!(run.gap_count, 1);
        assert!((run.coverage_fraction - 0.75).abs() < 1e-9);
        assert!(run.has_missing_coverage());
    }
}
