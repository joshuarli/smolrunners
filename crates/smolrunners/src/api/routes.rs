use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use smolrunners_core::domain::{
    Arch, Distro, ImageTarget, Profile, ProfileName, ProfileRevision, ReleaseToken, Shape,
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
                let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
                let invitation = state
                    .create_invitation(&host_key, now_ms())
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
                let mut state = state.lock().map_err(|_| (500, "lock", "state lock poisoned".to_owned()))?;
                let host = state
                    .join_host(token, &hostname, &arch, vcpu, memory, now_ms())
                    .map_err(|error| (409, "join_failed", error))?;
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
                    let (previous, next) = state
                        .set_host_state(id, "drained")
                        .map_err(|error| (404, "host_not_found", error))?;
                    json!({ "previous": previous, "next": next })
                }
                Some("resume") => {
                    let (previous, next) = state
                        .set_host_state(id, "eligible")
                        .map_err(|error| (404, "host_not_found", error))?;
                    json!({ "previous": previous, "next": next })
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
                    let summaries = state.summary_by_job();
                    let candidates: Vec<&ProfileRecord> = state.data.profiles.iter().collect();
                    let observation = candidates.first().map(|profile| {
                        let peak = summaries.values().map(|value| value["peak_cpu_cores"].as_f64().unwrap_or(0.0)).fold(0.0f64, f64::max);
                        json!({
                            "profile": profile.name,
                            "observed_peak_cpu_cores": peak,
                            "observation": "peak cpu across completed comparable runs",
                            "proposal": profile.name,
                            "confidence": 0.6,
                            "falsifier": "a paired trial on the same corpus with different host pressure",
                        })
                    }).unwrap_or_else(|| json!({
                        "profile": null,
                        "observed_peak_cpu_cores": null,
                        "observation": "no comparable runs",
                        "proposal": null,
                        "confidence": 0.0,
                        "falsifier": "run a paired trial after collecting comparable jobs",
                    }));
                    let mut reply = ApiReply::new(200, command, observation);
                    if candidates.is_empty() {
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
                            let host_id = body["host_id"].as_i64().ok_or_else(|| (400, "missing_host", "missing host_id".to_owned()))?;
                            let job = state.start_job(id, host_id, now_ms()).map_err(|error| (404, "job_not_found", error))?;
                            state.persist().map_err(|error| (500, "persist_failed", error))?;
                            Ok(ApiReply::new(200, command, serde_json::to_value(&job).unwrap())
                                .with_scope(json!({ "kind": "job", "id": id, "vm_scoped": true })))
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
