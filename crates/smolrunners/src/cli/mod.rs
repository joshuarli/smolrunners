mod args;
mod render;
mod transport;

use std::fs;

use serde_json::{Value, json};

use crate::api::ApiReply;

pub use render::Output;
pub use transport::{HttpTransport, Transport};

fn usage() -> Output {
    Output::ok(
        "smolrunners <command>\n\
         init, doctor\n\
         github connect\n\
         host invite, host join\n\
         hosts list, inspect, drain, resume, revoke, kill, heartbeat\n\
         profiles list, apply, inspect, recommend\n\
         images build, qualify, promote\n\
         caches list, inspect, prune\n\
         jobs list, inspect, top, metrics, logs, explain, report, compare, debug\n",
    )
}

pub fn run(args: &[String], transport: &dyn Transport) -> Output {
    match execute(args, transport) {
        Ok(output) => output,
        Err(message) => Output::error(message, 2),
    }
}

fn send(
    transport: &dyn Transport,
    method: &str,
    path: &str,
    body: Option<Value>,
    command: &str,
    json: bool,
    format: Option<&str>,
) -> Result<Output, String> {
    let payload = body.map(|value| value.to_string());
    let reply = transport.send(method, path, payload.as_deref())?;
    render::render(reply, command, json, format)
}

fn positional(parsed: &args::Parsed, index: usize) -> Option<&str> {
    parsed.command.get(index).map(String::as_str)
}

fn id_arg(parsed: &args::Parsed, index: usize, option: &str) -> Result<String, String> {
    positional(parsed, index)
        .map(str::to_owned)
        .or_else(|| parsed.option(option).map(str::to_owned))
        .ok_or_else(|| format!("missing id: pass a positional id or --{option}"))
}

fn profile_body(parsed: &args::Parsed) -> Result<Value, String> {
    if let Some(path) = parsed.option("file") {
        let text = fs::read_to_string(path).map_err(|error| format!("read profile {path}: {error}"))?;
        let document: toml::Value =
            toml::from_str(&text).map_err(|error| format!("parse profile {path}: {error}"))?;
        let field = |name: &str| {
            document
                .get(name)
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        };
        let integer = |name: &str| document.get(name).and_then(|value| value.as_integer());
        Ok(json!({
            "name": field("name"),
            "vcpu": integer("vcpu"),
            "memory_gib": integer("memory_gib"),
            "distro": field("distro"),
            "release": field("release"),
            "arch": field("arch"),
        }))
    } else {
        Ok(json!({
            "name": parsed.option("name"),
            "vcpu": parsed.option("vcpu").and_then(|value| value.parse::<u64>().ok()),
            "memory_gib": parsed.option("memory-gib").or_else(|| parsed.option("memory_gib")).and_then(|value| value.parse::<u64>().ok()),
            "distro": parsed.option("distro"),
            "release": parsed.option("release"),
            "arch": parsed.option("arch"),
        }))
    }
}

fn logs_reply(parsed: &args::Parsed) -> Output {
    let text = "unavailable; see GitHub";
    if parsed.has("json") {
        let reply = ApiReply::new(
            200,
            "jobs logs",
            json!({
                "available": false,
                "reason": text,
                "url": parsed.option("url"),
            }),
        )
        .missing("lines", "logs are collected by GitHub; no relay until NE1");
        match render::render(reply, "jobs logs", true, None) {
            Ok(output) => output,
            Err(error) => Output::error(error, 2),
        }
    } else {
        Output::ok(format!("{text}\n"))
    }
}

pub fn execute(args: &[String], transport: &dyn Transport) -> Result<Output, String> {
    let parsed = args::parse(args)?;
    let json = parsed.has("json");
    let format = parsed.option("format");
    let mut command: Vec<&str> = parsed.command.iter().map(String::as_str).collect();
    if command.len() > 2 {
        command.truncate(2);
    }

    match command.as_slice() {
        [] | ["help"] => Ok(usage()),
        ["init"] => send(
            transport,
            "POST",
            "/api/v1/init",
            Some(json!({})),
            "init",
            json,
            format,
        ),
        ["doctor"] => send(
            transport,
            "GET",
            "/api/v1/doctor",
            None,
            "doctor",
            json,
            format,
        ),
        ["github", "connect"] => {
            let app_id = parsed
                .option("app-id")
                .or_else(|| parsed.option("app_id"))
                .unwrap_or("1")
                .parse::<i64>()
                .map_err(|error| format!("bad --app-id: {error}"))?;
            let installation_id = parsed
                .option("installation-id")
                .or_else(|| parsed.option("installation_id"))
                .unwrap_or("1")
                .parse::<i64>()
                .map_err(|error| format!("bad --installation-id: {error}"))?;
            let account = parsed.option("account").unwrap_or("fixture");
            send(
                transport,
                "POST",
                "/api/v1/github/connect",
                Some(json!({
                    "app_id": app_id,
                    "installation_id": installation_id,
                    "account_login": account,
                    "fake": true,
                })),
                "github connect",
                json,
                format,
            )
        }
        ["host", "invite"] => send(
            transport,
            "POST",
            "/api/v1/host/invite",
            Some(json!({
                "host_key": parsed.option("host-key").unwrap_or("sr-host-a"),
                "cert_fingerprint": parsed.option("cert-fingerprint"),
            })),
            "host invite",
            json,
            format,
        ),
        ["host", "join"] => send(
            transport,
            "POST",
            "/api/v1/host/join",
            Some(json!({
                "token": parsed.option("token"),
                "hostname": parsed.option("hostname").unwrap_or("sr-host-a"),
                "arch": parsed.option("arch").unwrap_or("arm64"),
                "vcpu": parsed.option("vcpu").and_then(|value| value.parse::<u64>().ok()),
                "memory_gib": parsed.option("memory-gib").and_then(|value| value.parse::<u64>().ok()),
                "host_key": parsed.option("host-key").unwrap_or("sr-host-a"),
                "cert_fingerprint": parsed.option("cert-fingerprint").unwrap_or("fixture-client-cert"),
            })),
            "host join",
            json,
            format,
        ),
        ["hosts", "list"] => send(
            transport,
            "GET",
            "/api/v1/hosts",
            None,
            "hosts list",
            json,
            format,
        ),
        ["hosts", "inspect"] => {
            let id = id_arg(&parsed, 2, "id")?;
            send(
                transport,
                "GET",
                &format!("/api/v1/hosts/{id}"),
                None,
                "hosts inspect",
                json,
                format,
            )
        }
        ["hosts", "drain"] => {
            let id = id_arg(&parsed, 2, "id")?;
            send(
                transport,
                "POST",
                &format!("/api/v1/hosts/{id}/drain"),
                Some(json!({})),
                "hosts drain",
                json,
                format,
            )
        }
        ["hosts", "resume"] => {
            let id = id_arg(&parsed, 2, "id")?;
            send(
                transport,
                "POST",
                &format!("/api/v1/hosts/{id}/resume"),
                Some(json!({})),
                "hosts resume",
                json,
                format,
            )
        }
        ["hosts", "revoke"] => {
            let id = id_arg(&parsed, 2, "id")?;
            send(
                transport,
                "POST",
                &format!("/api/v1/hosts/{id}/revoke"),
                Some(json!({})),
                "hosts revoke",
                json,
                format,
            )
        }
        ["hosts", "kill"] => {
            let id = id_arg(&parsed, 2, "id")?;
            send(
                transport,
                "POST",
                &format!("/api/v1/hosts/{id}/kill"),
                Some(json!({})),
                "hosts kill",
                json,
                format,
            )
        }
        ["hosts", "heartbeat"] => {
            let id = id_arg(&parsed, 2, "id")?;
            send(
                transport,
                "POST",
                &format!("/api/v1/hosts/{id}/heartbeat"),
                Some(json!({ "incarnation": parsed.option("incarnation") })),
                "hosts heartbeat",
                json,
                format,
            )
        }
        ["profiles", "list"] => send(
            transport,
            "GET",
            "/api/v1/profiles",
            None,
            "profiles list",
            json,
            format,
        ),
        ["profiles", "apply"] => send(
            transport,
            "POST",
            "/api/v1/profiles/apply",
            Some(profile_body(&parsed)?),
            "profiles apply",
            json,
            format,
        ),
        ["profiles", "inspect"] => {
            let name = id_arg(&parsed, 2, "name")?;
            send(
                transport,
                "GET",
                &format!("/api/v1/profiles/{name}"),
                None,
                "profiles inspect",
                json,
                format,
            )
        }
        ["profiles", "recommend"] => send(
            transport,
            "GET",
            "/api/v1/profiles/recommend",
            None,
            "profiles recommend",
            json,
            format,
        ),
        ["profiles", "promote"] => {
            let name = id_arg(&parsed, 2, "name")?;
            send(
                transport,
                "POST",
                "/api/v1/profiles/promote",
                Some(json!({ "name": name })),
                "profiles promote",
                json,
                format,
            )
        }
        ["images", "build"] => send(
            transport,
            "POST",
            "/api/v1/images/build",
            Some(json!({
                "distro": parsed.option("distro").unwrap_or("debian"),
                "release": parsed.option("release").unwrap_or("13"),
                "arch": parsed.option("arch").unwrap_or("arm64"),
                "digest": parsed.option("digest").unwrap_or("sha256:fixture"),
            })),
            "images build",
            json,
            format,
        ),
        ["images", "qualify"] => {
            let id = id_arg(&parsed, 2, "id")?;
            send(
                transport,
                "POST",
                &format!("/api/v1/images/{id}/qualify"),
                Some(json!({ "qualified": !parsed.has("reject") })),
                "images qualify",
                json,
                format,
            )
        }
        ["images", "promote"] => {
            let id = id_arg(&parsed, 2, "id")?;
            send(
                transport,
                "POST",
                &format!("/api/v1/images/{id}/promote"),
                Some(json!({})),
                "images promote",
                json,
                format,
            )
        }
        ["caches", "list"] => send(
            transport,
            "GET",
            "/api/v1/caches",
            None,
            "caches list",
            json,
            format,
        ),
        ["caches", "inspect"] => {
            let id = id_arg(&parsed, 2, "id")?;
            send(
                transport,
                "GET",
                &format!("/api/v1/caches/{id}"),
                None,
                "caches inspect",
                json,
                format,
            )
        }
        ["caches", "prune"] => send(
            transport,
            "POST",
            "/api/v1/caches/prune",
            Some(json!({ "dry_run": !parsed.has("apply"), "non_interactive": parsed.has("non-interactive") })),
            "caches prune",
            json,
            format,
        ),
        ["jobs", "list"] => send(
            transport,
            "GET",
            "/api/v1/jobs",
            None,
            "jobs list",
            json,
            format,
        ),
        ["jobs", "inspect"] => {
            let id = id_arg(&parsed, 2, "id")?;
            send(
                transport,
                "GET",
                &format!("/api/v1/jobs/{id}"),
                None,
                "jobs inspect",
                json,
                format,
            )
        }
        ["jobs", "top"] => send(
            transport,
            "GET",
            "/api/v1/jobs/top",
            None,
            "jobs top",
            json,
            format,
        ),
        ["jobs", "metrics"] => {
            let id = resolve_job_id(transport, &parsed)?;
            send(
                transport,
                "GET",
                &format!("/api/v1/jobs/{id}/metrics"),
                None,
                "jobs metrics",
                json,
                format,
            )
        }
        ["jobs", "logs"] => Ok(logs_reply(&parsed)),
        ["jobs", "explain"] => {
            let id = resolve_job_id(transport, &parsed)?;
            send(
                transport,
                "GET",
                &format!("/api/v1/jobs/{id}/explain"),
                None,
                "jobs explain",
                json,
                format,
            )
        }
        ["jobs", "report"] => send(
            transport,
            "GET",
            "/api/v1/jobs/report",
            None,
            "jobs report",
            json,
            format,
        ),
        ["jobs", "compare"] => {
            let left = parsed.option("a").or_else(|| parsed.option("left")).unwrap_or("0");
            let right = parsed.option("b").or_else(|| parsed.option("right")).unwrap_or("0");
            send(
                transport,
                "POST",
                "/api/v1/jobs/compare",
                Some(json!({
                    "a": left.parse::<i64>().unwrap_or(0),
                    "b": right.parse::<i64>().unwrap_or(0),
                })),
                "jobs compare",
                json,
                format,
            )
        }
        ["jobs", "debug"] => {
            let id = resolve_job_id(transport, &parsed)?;
            send(
                transport,
                "GET",
                &format!("/api/v1/jobs/{id}/debug"),
                None,
                "jobs debug",
                json,
                format,
            )
        }
        other => Err(format!("unknown command: {}", other.join(" "))),
    }
}

fn resolve_job_id(transport: &dyn Transport, parsed: &args::Parsed) -> Result<String, String> {
    if let Some(id) = positional(parsed, 2) {
        return Ok(id.to_owned());
    }
    if let Some(id) = parsed.option("id") {
        return Ok(id.to_owned());
    }
    let reply = transport.send("GET", "/api/v1/jobs", None)?;
    let jobs = reply.data["jobs"].as_array().ok_or_else(|| "no jobs in response".to_owned())?;
    jobs.last()
        .and_then(|job| job.get("id"))
        .map(|id| id.to_string())
        .ok_or_else(|| "no jobs available; pass an id".to_owned())
}
