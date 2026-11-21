use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};
use smolrunners::api::ApiReply;
use smolrunners::cli::{Transport, execute};

#[derive(Default)]
struct FixtureTransport {
    replies: HashMap<String, ApiReply>,
    fallback: Option<ApiReply>,
}

impl FixtureTransport {
    fn one(reply: ApiReply) -> Self {
        Self {
            replies: HashMap::new(),
            fallback: Some(reply),
        }
    }

    fn route(mut self, method: &str, path: &str, reply: ApiReply) -> Self {
        self.replies.insert(format!("{method} {path}"), reply);
        self
    }
}

impl Transport for FixtureTransport {
    fn send(&self, method: &str, path: &str, _body: Option<&str>) -> Result<ApiReply, String> {
        self.replies
            .get(&format!("{method} {path}"))
            .or_else(|| self.fallback.as_ref())
            .cloned()
            .ok_or_else(|| format!("no fixture for {method} {path}"))
    }
}

fn expected_units() -> Value {
    json!({
        "bytes": "bytes",
        "confidence": "unit_interval",
        "count": "count",
        "cpu_cores": "cores",
        "cpu_percent": "percent",
        "duration_ms": "ms",
        "memory_gib": "gib",
        "ns": "ns",
        "peak_cpu_cores": "cores",
        "peak_memory_bytes": "bytes",
        "read_bytes": "bytes",
        "seconds": "seconds",
        "us": "us",
        "vcpu": "count",
        "write_bytes": "bytes",
    })
}

fn envelope(command: &str, data: Value, scope: Option<Value>, missing: Value) -> Value {
    let mut value = json!({
        "schema_version": 1,
        "command": command,
        "units": expected_units(),
        "missing": missing,
        "data": data,
    });
    if let Some(scope) = scope {
        value["scope"] = scope;
    }
    value
}

fn assert_json(
    args: &[&str],
    transport: &FixtureTransport,
    expected: Value,
) {
    let args: Vec<String> = args.iter().map(|value| (*value).to_owned()).collect();
    let output = execute(&args, transport).expect("execute command");
    assert_eq!(output.exit, 0, "stderr: {}", output.stderr);
    let actual: Value = serde_json::from_str(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid JSON {}: {error}", output.stdout));
    assert_eq!(actual, expected, "command {:?}", args);
}

fn missing_of(reply: &ApiReply) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in &reply.missing {
        map.insert(key.clone(), json!(value));
    }
    Value::Object(map)
}

#[test]
fn cli_init_json_has_version_units_and_scope() {
    let reply = ApiReply::new(200, "init", json!({ "initialized": true }))
        .with_scope(json!({ "kind": "coordinator", "path": "/state/telemetry.sqlite" }));
    let expected = envelope(
        "init",
        json!({ "initialized": true }),
        Some(json!({ "kind": "coordinator", "path": "/state/telemetry.sqlite" })),
        json!({}),
    );
    assert_json(&["smolrunners", "init", "--non-interactive", "--json"], &FixtureTransport::one(reply), expected);
}

#[test]
fn cli_doctor_json_explicitly_marks_unchecked_prerequisite() {
    let reply = ApiReply::new(200, "doctor", json!({
        "eligible": true,
        "checks": [
            { "name": "architecture", "verdict": "pass", "reason": null },
            { "name": "kvm", "verdict": "pass", "reason": null },
            { "name": "cgroup_v2", "verdict": "pass", "reason": null }
        ],
    }))
    .missing("reflink", "not checked by the API doctor");
    let expected = envelope(
        "doctor",
        reply.data.clone(),
        None,
        json!({ "reflink": "not checked by the API doctor" }),
    );
    assert_json(&["smolrunners", "doctor", "--json"], &FixtureTransport::one(reply), expected);
}

#[test]
fn cli_github_connect_json_is_fake_and_scoped() {
    let installation = json!({
        "app_id": 1,
        "installation_id": 9,
        "account_login": "fixture",
        "scope": "all",
        "runner_group": "default",
        "fake": true,
        "connected_at_ms": 1000,
    });
    let reply = ApiReply::new(200, "github connect", installation.clone())
        .with_scope(json!({ "kind": "installation", "id": 9, "fake": true }));
    let expected = envelope(
        "github connect",
        installation,
        Some(json!({ "kind": "installation", "id": 9, "fake": true })),
        json!({}),
    );
    assert_json(
        &["smolrunners", "github", "connect", "--fake", "--non-interactive", "--json"],
        &FixtureTransport::one(reply),
        expected,
    );
}

#[test]
fn cli_host_invite_and_join_json_show_scope() {
    let invitation = json!({
        "id": 1,
        "token": "sr-invite-1-1000",
        "host_key": "sr-host-a",
        "single_use": true,
        "consumed": false,
        "created_at_ms": 1000,
    });
    let invite_reply = ApiReply::new(200, "host invite", invitation.clone())
        .with_scope(json!({ "kind": "invitation", "id": 1, "single_use": true }));
    let invite_expected = envelope(
        "host invite",
        invitation,
        Some(json!({ "kind": "invitation", "id": 1, "single_use": true })),
        json!({}),
    );
    assert_json(
        &["smolrunners", "host", "invite", "--non-interactive", "--json"],
        &FixtureTransport::one(invite_reply),
        invite_expected,
    );

    let host = json!({
        "id": 2,
        "host_key": "sr-host-a",
        "hostname": "sr-host-a",
        "arch": "arm64",
        "total_vcpu": 4,
        "total_memory_gib": 8,
        "admission_state": "eligible",
        "incarnation": 1,
        "capabilities": [],
        "last_contact_ms": 1000,
    });
    let join_reply = ApiReply::new(200, "host join", host.clone())
        .with_scope(json!({ "kind": "host", "id": 2, "incarnation": 1 }));
    let join_expected = envelope(
        "host join",
        host,
        Some(json!({ "kind": "host", "id": 2, "incarnation": 1 })),
        json!({}),
    );
    assert_json(
        &["smolrunners", "host", "join", "--token", "sr-invite-1-1000", "--non-interactive", "--json"],
        &FixtureTransport::one(join_reply),
        join_expected,
    );
}

#[test]
fn cli_hosts_list_inspect_drain_resume_json() {
    let host = json!({
        "id": 2,
        "host_key": "sr-host-a",
        "hostname": "sr-host-a",
        "arch": "arm64",
        "total_vcpu": 4,
        "total_memory_gib": 8,
        "admission_state": "eligible",
        "incarnation": 1,
        "capabilities": [],
        "last_contact_ms": null,
    });
    let list_reply = ApiReply::new(200, "hosts list", json!({ "hosts": [host.clone()] }))
        .missing("last_contact_ms", "no host has contacted the coordinator");
    assert_json(
        &["smolrunners", "hosts", "list", "--json"],
        &FixtureTransport::one(list_reply),
        envelope(
            "hosts list",
            json!({ "hosts": [host.clone()] }),
            None,
            json!({ "last_contact_ms": "no host has contacted the coordinator" }),
        ),
    );

    let inspect_reply = ApiReply::new(200, "hosts inspect", host.clone())
        .missing("last_contact_ms", "host has never contacted");
    assert_json(
        &["smolrunners", "hosts", "inspect", "2", "--json"],
        &FixtureTransport::one(inspect_reply),
        envelope(
            "hosts inspect",
            host,
            None,
            json!({ "last_contact_ms": "host has never contacted" }),
        ),
    );

    for (action, previous, next) in [("drain", "eligible", "drained"), ("resume", "drained", "eligible")] {
        let reply = ApiReply::new(200, &format!("hosts {action}"), json!({ "previous": previous, "next": next }))
            .with_scope(json!({ "kind": "host", "id": 2 }));
        assert_json(
            &["smolrunners", "hosts", action, "2", "--non-interactive", "--json"],
            &FixtureTransport::one(reply),
            envelope(
                &format!("hosts {action}"),
                json!({ "previous": previous, "next": next }),
                Some(json!({ "kind": "host", "id": 2 })),
                json!({}),
            ),
        );
    }
}

#[test]
fn cli_profiles_list_apply_inspect_recommend_json() {
    let profile = json!({
        "name": "balanced-4c8g",
        "revision_id": "22a3b1d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091",
        "revision_number": 1,
        "vcpu": 4,
        "memory_gib": 8,
        "family": "balanced",
        "distro": "debian",
        "release": "13",
        "arch": "arm64",
        "promoted": false,
        "applied_at_ms": 1000,
    });
    let list_reply = ApiReply::new(200, "profiles list", json!({ "profiles": [profile.clone()] }));
    assert_json(
        &["smolrunners", "profiles", "list", "--json"],
        &FixtureTransport::one(list_reply),
        envelope("profiles list", json!({ "profiles": [profile.clone()] }), None, json!({})),
    );

    let apply_reply = ApiReply::new(200, "profiles apply", profile.clone())
        .with_scope(json!({ "kind": "profile", "name": "balanced-4c8g", "revision": 1 }));
    assert_json(
        &["smolrunners", "profiles", "apply", "--file", concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/cli/fixtures/profile.toml"), "--non-interactive", "--json"],
        &FixtureTransport::one(apply_reply),
        envelope(
            "profiles apply",
            profile.clone(),
            Some(json!({ "kind": "profile", "name": "balanced-4c8g", "revision": 1 })),
            json!({}),
        ),
    );

    let inspect_reply = ApiReply::new(200, "profiles inspect", profile.clone());
    assert_json(
        &["smolrunners", "profiles", "inspect", "balanced-4c8g", "--json"],
        &FixtureTransport::one(inspect_reply),
        envelope("profiles inspect", profile, None, json!({})),
    );

    let recommendation = json!({
        "profile": "balanced-4c8g",
        "observed_peak_cpu_cores": 1.5,
        "observation": "peak cpu across completed comparable runs",
        "proposal": "balanced-4c8g",
        "confidence": 0.6,
        "falsifier": "a paired trial on the same corpus with different host pressure",
    });
    let recommend_reply = ApiReply::new(200, "profiles recommend", recommendation.clone());
    assert_json(
        &["smolrunners", "profiles", "recommend", "--json"],
        &FixtureTransport::one(recommend_reply),
        envelope("profiles recommend", recommendation, None, json!({})),
    );
}

#[test]
fn cli_images_build_qualify_promote_json() {
    let image = json!({
        "id": 3,
        "distro": "debian",
        "release": "13",
        "arch": "arm64",
        "digest": "sha256:fixture",
        "qualified": false,
        "promoted": false,
        "built_at_ms": 1000,
    });
    let build_reply = ApiReply::new(200, "images build", image.clone())
        .with_scope(json!({ "kind": "image", "id": 3 }));
    assert_json(
        &["smolrunners", "images", "build", "--non-interactive", "--json"],
        &FixtureTransport::one(build_reply),
        envelope("images build", image, Some(json!({ "kind": "image", "id": 3 })), json!({})),
    );

    let qualify_reply = ApiReply::new(200, "images qualify", json!({ "id": 3, "qualified": true }))
        .with_scope(json!({ "kind": "image", "id": 3 }));
    assert_json(
        &["smolrunners", "images", "qualify", "3", "--non-interactive", "--json"],
        &FixtureTransport::one(qualify_reply),
        envelope(
            "images qualify",
            json!({ "id": 3, "qualified": true }),
            Some(json!({ "kind": "image", "id": 3 })),
            json!({}),
        ),
    );

    let promote_reply = ApiReply::new(200, "images promote", json!({ "id": 3, "promoted": true }))
        .with_scope(json!({ "kind": "image", "id": 3 }));
    assert_json(
        &["smolrunners", "images", "promote", "3", "--non-interactive", "--json"],
        &FixtureTransport::one(promote_reply),
        envelope(
            "images promote",
            json!({ "id": 3, "promoted": true }),
            Some(json!({ "kind": "image", "id": 3 })),
            json!({}),
        ),
    );
}

#[test]
fn cli_caches_list_inspect_prune_dry_run_json() {
    let cache = json!({
        "id": 4,
        "kind": "action-archive",
        "scope": "repo:o/r",
        "generation": 3,
        "lineage": ["gen-1", "gen-2"],
        "size_bytes": 1024,
        "pinned": false,
        "active": false,
        "created_at_ms": 1000,
    });
    let list_reply = ApiReply::new(200, "caches list", json!({ "caches": [cache.clone()] }));
    assert_json(
        &["smolrunners", "caches", "list", "--json"],
        &FixtureTransport::one(list_reply),
        envelope("caches list", json!({ "caches": [cache.clone()] }), None, json!({})),
    );

    let inspect_reply = ApiReply::new(200, "caches inspect", cache.clone());
    assert_json(
        &["smolrunners", "caches", "inspect", "4", "--json"],
        &FixtureTransport::one(inspect_reply),
        envelope("caches inspect", cache, None, json!({})),
    );

    let prune_reply = ApiReply::new(200, "caches prune", json!({ "dry_run": true, "candidates": [4] }))
        .with_scope(json!({ "kind": "cache", "dry_run": true }));
    assert_json(
        &["smolrunners", "caches", "prune", "--dry-run", "--non-interactive", "--json"],
        &FixtureTransport::one(prune_reply),
        envelope(
            "caches prune",
            json!({ "dry_run": true, "candidates": [4] }),
            Some(json!({ "kind": "cache", "dry_run": true })),
            json!({}),
        ),
    );
}

fn job() -> Value {
    json!({
        "id": 5,
        "execution_attempt_id": 6,
        "protocol_job_id": "evt-job-1",
        "request_id": 1,
        "job_key": "build",
        "name": "build",
        "profile": "small-1c2g",
        "state": "done",
        "host_id": 2,
        "labels": ["smol-debian13-arm64-1c2g"],
        "demand_at_ms": 1000,
        "start_at_ms": 1100,
        "finish_at_ms": 3000,
        "result": "success",
        "admission_reason": "profile matched advertised labels",
        "github_url": null,
    })
}

#[test]
fn cli_jobs_list_inspect_top_json() {
    let job = job();
    let list_reply = ApiReply::new(200, "jobs list", json!({ "jobs": [job.clone()] }));
    assert_json(
        &["smolrunners", "jobs", "list", "--json"],
        &FixtureTransport::one(list_reply),
        envelope("jobs list", json!({ "jobs": [job.clone()] }), None, json!({})),
    );

    let inspect_reply = ApiReply::new(200, "jobs inspect", job.clone());
    assert_json(
        &["smolrunners", "jobs", "inspect", "5", "--json"],
        &FixtureTransport::one(inspect_reply),
        envelope("jobs inspect", job.clone(), None, json!({})),
    );

    let top_reply = ApiReply::new(200, "jobs top", json!({ "jobs": [job.clone()], "rank_by": "peak_cpu_cores" }));
    assert_json(
        &["smolrunners", "jobs", "top", "--json"],
        &FixtureTransport::one(top_reply),
        envelope("jobs top", json!({ "jobs": [job], "rank_by": "peak_cpu_cores" }), None, json!({})),
    );
}

#[test]
fn cli_jobs_metrics_follow_json_has_units_and_explicit_gaps() {
    let job = job();
    let jobs_reply = ApiReply::new(200, "jobs list", json!({ "jobs": [job.clone()] }));
    let metrics_reply = ApiReply::new(200, "jobs metrics 5", json!({
        "id": 5,
        "execution_attempt_id": 6,
        "samples": 2,
        "series": [
            { "scope": "guest", "metric": "cpu.busy_cores", "value": 0.5, "unit": "cores", "sequence": 1, "captured_at": "2026-11-21T00:00:00.000Z" },
            { "scope": "guest", "metric": "cpu.busy_cores", "value": 0.75, "unit": "cores", "sequence": 3, "captured_at": "2026-11-21T00:00:01.000Z" },
        ],
    }))
    .missing("gap", "sequence gap between samples 1 and 3");
    let transport = FixtureTransport::default()
        .route("GET", "/api/v1/jobs", jobs_reply)
        .route("GET", "/api/v1/jobs/5/metrics", metrics_reply);
    let args: Vec<String> = ["smolrunners", "jobs", "metrics", "--follow", "--json"]
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    let output = execute(&args, &transport).expect("metrics");
    assert_eq!(output.exit, 0, "stderr: {}", output.stderr);
    let actual: Value = serde_json::from_str(&output.stdout).expect("json");
    assert_eq!(actual["schema_version"], 1);
    assert_eq!(actual["command"], "jobs metrics 5");
    assert_eq!(actual["units"]["cpu_cores"], "cores");
    assert_eq!(actual["units"]["cpu_percent"], "percent");
    assert_eq!(
        actual["missing"]["gap"],
        "sequence gap between samples 1 and 3"
    );
    assert_eq!(actual["data"]["samples"], 2);
    assert_eq!(actual["data"]["series"][1]["unit"], "cores");
    let _ = missing_of(&ApiReply::new(200, "x", json!({})));
}

#[test]
fn cli_jobs_logs_follow_reports_unavailable() {
    let args: Vec<String> = ["smolrunners", "jobs", "logs", "--follow"]
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    let output = execute(&args, &FixtureTransport::default()).expect("logs");
    assert_eq!(output.exit, 0);
    assert_eq!(output.stdout, "unavailable; see GitHub\n");

    let json_args: Vec<String> = ["smolrunners", "jobs", "logs", "--follow", "--json"]
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    let output = execute(&json_args, &FixtureTransport::default()).expect("logs json");
    let actual: Value = serde_json::from_str(&output.stdout).expect("json");
    assert_eq!(actual["schema_version"], 1);
    assert_eq!(actual["data"]["available"], false);
    assert_eq!(actual["data"]["reason"], "unavailable; see GitHub");
    assert!(actual["missing"]["lines"].is_string());
}

#[test]
fn cli_jobs_explain_compare_debug_json() {
    let explain = json!({
        "id": 5,
        "state": "done",
        "profile": "small-1c2g",
        "admission_reason": "profile matched advertised labels",
        "host_id": 2,
        "labels": ["smol-debian13-arm64-1c2g"],
    });
    let explain_reply = ApiReply::new(200, "jobs explain 5", explain.clone());
    assert_json(
        &["smolrunners", "jobs", "explain", "5", "--json"],
        &FixtureTransport::one(explain_reply),
        envelope("jobs explain 5", explain, None, json!({})),
    );

    let compare = json!({
        "left": { "id": 5, "profile": "small-1c2g", "summary": { "peak_cpu_cores": 0.75 } },
        "right": null,
    });
    let compare_reply = ApiReply::new(200, "jobs compare", compare.clone());
    assert_json(
        &["smolrunners", "jobs", "compare", "--a", "5", "--b", "6", "--json"],
        &FixtureTransport::one(compare_reply),
        envelope("jobs compare", compare, None, json!({})),
    );

    let debug = json!({
        "id": 5,
        "vm_scoped": true,
        "authenticated": true,
        "time_bounded_ms": 2000,
        "state": "done",
        "host_id": 2,
    });
    let debug_reply = ApiReply::new(200, "jobs debug 5", debug.clone())
        .with_scope(json!({ "kind": "vm", "execution_attempt_id": 6 }));
    assert_json(
        &["smolrunners", "jobs", "debug", "5", "--json"],
        &FixtureTransport::one(debug_reply),
        envelope(
            "jobs debug 5",
            debug,
            Some(json!({ "kind": "vm", "execution_attempt_id": 6 })),
            json!({}),
        ),
    );
}

#[test]
fn cli_jobs_report_format_csv() {
    let reply = ApiReply::new(200, "jobs report", json!({
        "rows": [
            { "id": 5, "name": "build", "state": "done", "profile": "small-1c2g", "peak_cpu_cores": 0.75, "peak_memory_bytes": 1048576.0, "samples": 2 }
        ],
    }));
    let args: Vec<String> = ["smolrunners", "jobs", "report", "--format", "csv"]
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    let output = execute(&args, &FixtureTransport::one(reply)).expect("report");
    assert_eq!(output.exit, 0);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0],
        "id,name,peak_cpu_cores,peak_memory_bytes,profile,samples,state"
    );
    assert!(lines[1].contains("build"));
    assert!(lines[1].contains("0.75"));
    let _ = BTreeMap::<String, String>::new();
}
