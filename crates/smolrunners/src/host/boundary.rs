use crate::host::config::Config;
use crate::host::environment::is_policy_variable;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const SCHEMA_VERSION: u32 = 1;
pub const RUNTIME_VERSION: &str = "1.36.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub verdict: Verdict,
    pub evidence: Value,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub kind: String,
    pub eligible: bool,
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone)]
pub struct BoundaryInput {
    pub uid: u32,
    pub cpu_max: String,
    pub memory_max: String,
    pub pids_max: String,
    pub net_allow: String,
    pub net_mode: String,
    pub machine: String,
    pub cgroup: String,
    pub incarnation: String,
    pub vmm_pid: Option<u32>,
    pub policy_variables: Vec<String>,
}

impl BoundaryInput {
    pub fn from_config(config: &Config) -> Self {
        Self {
            uid: config.uid,
            cpu_max: config.cpu_max.clone(),
            memory_max: config.memory_max.clone(),
            pids_max: config.pids_max.clone(),
            net_allow: config.net_allow.clone(),
            net_mode: config.net_mode(),
            machine: String::new(),
            cgroup: String::new(),
            incarnation: String::new(),
            vmm_pid: None,
            policy_variables: Vec::new(),
        }
    }
}

fn check(name: &str, verdict: Verdict, evidence: Value, reason: Option<String>) -> Check {
    Check {
        name: name.to_string(),
        verdict,
        evidence,
        reason,
    }
}

fn finish(kind: &str, checks: Vec<Check>) -> Report {
    Report {
        schema_version: SCHEMA_VERSION,
        kind: kind.to_string(),
        eligible: !checks.iter().any(|item| item.verdict == Verdict::Fail),
        checks,
    }
}

pub fn report_path(state: &Path) -> PathBuf {
    state.join("boundary.json")
}

impl Report {
    pub fn write(&self, state: &Path) -> Result<(), String> {
        fs::create_dir_all(state).map_err(|error| format!("create state: {error}"))?;
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("encode boundary report: {error}"))?;
        let path = report_path(state);
        let temporary = state.join(format!(".boundary.{}.tmp", std::process::id()));
        fs::write(&temporary, encoded).map_err(|error| format!("write {}: {error}", temporary.display()))?;
        fs::rename(&temporary, &path).map_err(|error| format!("rename {}: {error}", path.display()))
    }

    pub fn read(state: &Path) -> Result<Self, String> {
        let path = report_path(state);
        let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|error| format!("parse {}: {error}", path.display()))
    }
}

fn proc_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn proc_status(pid: u32, field: &str) -> Option<String> {
    let bytes = fs::read(format!("/proc/{pid}/status")).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines() {
        if let Some(value) = line.strip_prefix(field) {
            return Some(value.trim().to_string());
        }
    }
    None
}

fn proc_cgroup(pid: u32) -> Option<String> {
    let bytes = fs::read(format!("/proc/{pid}/cgroup")).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("0::") {
            return Some(value.trim().to_string());
        }
    }
    None
}

fn proc_environ(pid: u32, name: &str) -> Option<String> {
    let bytes = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let prefix = format!("{name}=");
    for entry in bytes.split(|byte| *byte == 0) {
        if let Ok(text) = std::str::from_utf8(entry) {
            if let Some(value) = text.strip_prefix(&prefix) {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn cgroup_file(cgroup: &str, name: &str) -> Option<String> {
    let path = format!("/sys/fs/cgroup/{cgroup}/{name}");
    let bytes = fs::read(&path).ok()?;
    Some(String::from_utf8_lossy(&bytes).trim().to_string())
}

fn cgroup_name(cgroup: &str) -> &str {
    cgroup.trim_start_matches("/sys/fs/cgroup/")
}

fn is_loopback(cidr: &str) -> bool {
    let lower = cidr.to_ascii_lowercase();
    lower.starts_with("127.")
        || lower.starts_with("::1")
        || lower == "0.0.0.0/0"
        || lower == "::/0"
        || lower.starts_with("169.254.")
        || lower.starts_with("fe80::")
}

fn is_private_v4(cidr: &str) -> bool {
    let lower = cidr.to_ascii_lowercase();
    lower.starts_with("10.")
        || lower.starts_with("192.168.")
        || (lower.starts_with("172.")
            && lower
                .split('.')
                .nth(1)
                .and_then(|value| value.parse::<u32>().ok())
                .is_some_and(|value| (16..=31).contains(&value)))
}

fn parse_cidrs(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

pub fn evaluate(input: &BoundaryInput) -> Report {
    let mut checks = Vec::new();
    let live = input.vmm_pid.is_some() && proc_alive(input.vmm_pid.unwrap_or(0));
    let pid = input.vmm_pid.unwrap_or(0);

    let pid_evidence = json!({
        "vmm_pid": input.vmm_pid,
        "incarnation": if input.incarnation.is_empty() { Value::Null } else { Value::String(input.incarnation.clone()) },
        "machine": if input.machine.is_empty() { Value::Null } else { Value::String(input.machine.clone()) },
        "alive": live,
        "source": "marker and /proc"
    });
    if live && !input.incarnation.is_empty() {
        checks.push(check("vmm_pid", Verdict::Pass, pid_evidence, None));
    } else if input.vmm_pid.is_some() {
        checks.push(check(
            "vmm_pid",
            Verdict::Fail,
            pid_evidence,
            Some("vmm process is not alive or incarnation is missing".to_string()),
        ));
    } else {
        checks.push(check(
            "vmm_pid",
            Verdict::Missing,
            pid_evidence,
            Some("no active vmm; start a host job to inspect its pid".to_string()),
        ));
    }

    let uid_observed = if live {
        proc_status(pid, "Uid").and_then(|value| value.split_whitespace().nth(2).map(str::to_string))
    } else {
        None
    };
    let uid_evidence = json!({
        "expected": input.uid,
        "observed": uid_observed,
        "vmm_pid": input.vmm_pid,
        "source": format!("/proc/{pid}/status:Uid")
    });
    if let Some(observed) = uid_observed.as_deref() {
        if observed.parse::<u32>().ok() == Some(input.uid) {
            checks.push(check("per_job_uid", Verdict::Pass, uid_evidence, None));
        } else {
            checks.push(check(
                "per_job_uid",
                Verdict::Fail,
                uid_evidence,
                Some(format!("effective uid {observed} is not the configured uid {}", input.uid)),
            ));
        }
    } else if live {
        checks.push(check(
            "per_job_uid",
            Verdict::Fail,
            uid_evidence,
            Some("cannot read effective uid from /proc".to_string()),
        ));
    } else {
        checks.push(check(
            "per_job_uid",
            Verdict::Missing,
            uid_evidence,
            Some("no active vmm to inspect for uid".to_string()),
        ));
    }

    let cgroup_observed = if live { proc_cgroup(pid) } else { None };
    let expected_cgroup = cgroup_name(&input.cgroup).to_string();
    let cgroup_evidence = json!({
        "expected": expected_cgroup,
        "observed": cgroup_observed,
        "vmm_pid": input.vmm_pid,
        "source": format!("/proc/{pid}/cgroup")
    });
    match cgroup_observed.as_deref() {
        Some(observed) if observed == format!("/{expected_cgroup}") || observed.ends_with(&format!("/{expected_cgroup}")) => {
            checks.push(check("cgroup_membership", Verdict::Pass, cgroup_evidence, None));
        }
        Some(_) => checks.push(check(
            "cgroup_membership",
            Verdict::Fail,
            cgroup_evidence,
            Some("vmm is not in the owned sr- cgroup".to_string()),
        )),
        None if live => checks.push(check(
            "cgroup_membership",
            Verdict::Fail,
            cgroup_evidence,
            Some("cannot read vmm cgroup membership".to_string()),
        )),
        None => checks.push(check(
            "cgroup_membership",
            Verdict::Missing,
            cgroup_evidence,
            Some("no active vmm to inspect for cgroup membership".to_string()),
        )),
    }

    for (name, expected, default) in [
        ("cpu_max", &input.cpu_max, "max 100000"),
        ("memory_max", &input.memory_max, "max"),
        ("pids_max", &input.pids_max, "max"),
    ] {
        if input.cgroup.is_empty() {
            checks.push(check(
                name,
                Verdict::Missing,
                json!({"path": Value::Null, "expected": expected, "observed": Value::Null}),
                Some("no owned cgroup to inspect for limits".to_string()),
            ));
            continue;
        }
        let observed = cgroup_file(&input.cgroup, name);
        let evidence = json!({
            "path": format!("{}/{}", input.cgroup, name),
            "expected": expected,
            "observed": observed,
            "default": default
        });
        let stripped = expected.trim().is_empty()
            || expected.trim() == default
            || observed.as_deref().is_none_or(|value| value.trim().is_empty());
        if !stripped && observed.as_deref() == Some(expected.as_str()) {
            checks.push(check(name, Verdict::Pass, evidence, None));
        } else {
            let reason = if stripped {
                format!("{name} is absent or left at the kernel default {default}")
            } else {
                format!("{name} is {} but expected {}", observed.as_deref().unwrap_or("absent"), expected)
            };
            checks.push(check(name, Verdict::Fail, evidence, Some(reason)));
        }
    }

    let landlock_env = if live { proc_environ(pid, "SMOLVM_LANDLOCK") } else { None };
    let no_new_privs = if live { proc_status(pid, "NoNewPrivs") } else { None };
    let landlock_evidence = json!({
        "SMOLVM_LANDLOCK": landlock_env,
        "no_new_privs": no_new_privs,
        "vmm_pid": input.vmm_pid,
        "source": format!("/proc/{pid}/environ and /proc/{pid}/status")
    });
    if live
        && landlock_env.as_deref() == Some("enforce")
        && no_new_privs.as_deref() == Some("1")
    {
        checks.push(check("landlock", Verdict::Pass, landlock_evidence, None));
    } else if live {
        checks.push(check(
            "landlock",
            Verdict::Fail,
            landlock_evidence,
            Some("vmm did not boot under Landlock enforce mode with no_new_privs set".to_string()),
        ));
    } else {
        checks.push(check(
            "landlock",
            Verdict::Missing,
            landlock_evidence,
            Some("no active vmm to inspect for Landlock configuration".to_string()),
        ));
    }

    let seccomp_env = if live { proc_environ(pid, "SMOLVM_SECCOMP") } else { None };
    let seccomp_mode = if live { proc_status(pid, "Seccomp") } else { None };
    let seccomp_evidence = json!({
        "SMOLVM_SECCOMP": seccomp_env,
        "seccomp": seccomp_mode,
        "vmm_pid": input.vmm_pid,
        "source": format!("/proc/{pid}/environ and /proc/{pid}/status")
    });
    let seccomp_filter = seccomp_mode
        .as_deref()
        .and_then(|value| value.parse::<u32>().ok())
        .is_some_and(|value| value >= 2);
    if live && seccomp_env.as_deref() == Some("enforce") && seccomp_filter {
        checks.push(check("seccomp", Verdict::Pass, seccomp_evidence, None));
    } else if live {
        checks.push(check(
            "seccomp",
            Verdict::Fail,
            seccomp_evidence,
            Some("vmm is not running with an enforced seccomp filter".to_string()),
        ));
    } else {
        checks.push(check(
            "seccomp",
            Verdict::Missing,
            seccomp_evidence,
            Some("no active vmm to inspect for seccomp filter".to_string()),
        ));
    }

    let cidrs = parse_cidrs(&input.net_allow);
    let vmm_net_allow = if live {
        proc_environ(pid, "SMOLRUNNERS_NET_ALLOW")
    } else {
        None
    };
    let loopback_allowed = input.net_mode == "allow-loopback"
        || cidrs.iter().any(|cidr| is_loopback(cidr))
        || vmm_net_allow.as_deref().is_some_and(|value| parse_cidrs(value).iter().any(|cidr| is_loopback(cidr)));
    let valid_v4 = cidrs.iter().any(|cidr| is_private_v4(cidr));
    let private_flag = input.net_mode == "private";

    let v4_evidence = json!({
        "net_allow": input.net_allow,
        "cidrs": cidrs,
        "vmm_net_allow": vmm_net_allow,
        "mode": input.net_mode,
        "block_private": private_flag,
        "host_loopback_allowed": loopback_allowed
    });
    if private_flag && valid_v4 && !loopback_allowed {
        checks.push(check("network_ipv4", Verdict::Pass, v4_evidence, None));
    } else {
        checks.push(check(
            "network_ipv4",
            Verdict::Fail,
            v4_evidence,
            Some("ipv4 policy is not restricted to private ranges or host loopback is reachable".to_string()),
        ));
    }

    let v6_evidence = json!({
        "mode": input.net_mode,
        "block_private": private_flag,
        "ipv6_loopback": loopback_allowed,
        "ipv6_link_local_blocked": private_flag,
        "source": "smolvm --block-private and vmm environment"
    });
    if private_flag && !loopback_allowed {
        checks.push(check("network_ipv6", Verdict::Pass, v6_evidence, None));
    } else {
        checks.push(check(
            "network_ipv6",
            Verdict::Fail,
            v6_evidence,
            Some("ipv6 private and link-local ranges are not blocked or host loopback is reachable".to_string()),
        ));
    }

    let dns_evidence = json!({
        "mode": input.net_mode,
        "resolver": "private guest gateway",
        "host_loopback_dns": loopback_allowed,
        "private_dns_policy": private_flag,
        "source": "smolvm --block-private and vmm environment"
    });
    if private_flag && !loopback_allowed {
        checks.push(check("network_dns", Verdict::Pass, dns_evidence, None));
    } else {
        checks.push(check(
            "network_dns",
            Verdict::Fail,
            dns_evidence,
            Some("dns policy is not private or host loopback dns is reachable".to_string()),
        ));
    }

    let environment_evidence = json!({
        "policy_variables": input.policy_variables,
        "source": "std::env at readiness evaluation"
    });
    if input.policy_variables.is_empty() {
        checks.push(check("policy_environment", Verdict::Pass, environment_evidence, None));
    } else {
        checks.push(check(
            "policy_environment",
            Verdict::Fail,
            environment_evidence,
            Some(format!(
                "policy variables present in host environment: {}",
                input.policy_variables.join(", ")
            )),
        ));
    }

    finish("readiness", checks)
}

pub fn host_policy_variables() -> Vec<String> {
    let mut found: Vec<String> = std::env::vars()
        .map(|(name, _)| name)
        .filter(|name| is_policy_variable(name))
        .collect();
    found.sort();
    found
}

fn missing(name: &str, evidence: Value, prerequisite: &str) -> Check {
    check(name, Verdict::Missing, evidence, Some(prerequisite.to_string()))
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|value| value.trim().to_string())
}

pub fn extras(config: &Config) -> Vec<Check> {
    let mut checks = Vec::new();

    let kvm = Path::new("/dev/kvm").exists();
    checks.push(check(
        "kvm_access",
        if kvm { Verdict::Pass } else { Verdict::Fail },
        json!({"device": "/dev/kvm", "present": kvm}),
        if kvm { None } else { Some("missing /dev/kvm; nested virtualization is required".to_string()) },
    ));

    let arch = std::env::consts::ARCH;
    let supported = matches!(arch, "aarch64" | "x86_64");
    checks.push(check(
        "architecture",
        if supported { Verdict::Pass } else { Verdict::Fail },
        json!({"arch": arch, "supported": supported}),
        if supported { None } else { Some(format!("unsupported architecture {arch}")) },
    ));

    let controllers = read_trimmed("/sys/fs/cgroup/cgroup.controllers").unwrap_or_default();
    let delegated: Vec<&str> = controllers
        .split_whitespace()
        .filter(|name| matches!(*name, "cpu" | "memory" | "pids"))
        .collect();
    let delegated_ok = delegated.len() == 3;
    checks.push(check(
        "cgroup_delegation",
        if delegated_ok { Verdict::Pass } else { Verdict::Fail },
        json!({"controllers": controllers, "required": ["cpu", "memory", "pids"]}),
        if delegated_ok { None } else { Some("cgroup v2 cpu, memory and pids controllers are not delegated".to_string()) },
    ));

    let scratch = config.state.join("doctor");
    let reflink_result = fs::create_dir_all(&scratch)
        .map_err(|error| error.to_string())
        .and_then(|_| {
            let source = scratch.join("reflink-source");
            let target = scratch.join("reflink-target");
            fs::write(&source, b"smolrunners doctor reflink probe").map_err(|error| error.to_string())?;
            let _ = fs::remove_file(&target);
            let output = Command::new("cp")
                .args(["--reflink=always", &source.to_string_lossy(), &target.to_string_lossy()])
                .output()
                .map_err(|error| error.to_string())?;
            let ok = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let _ = fs::remove_file(&source);
            let _ = fs::remove_file(&target);
            Ok((ok, stderr))
        });
    match reflink_result {
        Ok((true, _)) => checks.push(check(
            "reflink",
            Verdict::Pass,
            json!({"supported": true, "probe": "cp --reflink=always"}),
            None,
        )),
        Ok((false, stderr)) => {
            let filesystem = Command::new("stat")
                .args(["-f", "-c", "%T", &scratch.to_string_lossy()])
                .output()
                .ok()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            checks.push(missing(
                "reflink",
                json!({"supported": false, "filesystem": filesystem, "detail": stderr}),
                &format!("reflink is unavailable on filesystem {filesystem}; a reflink-capable filesystem is required"),
            ));
        }
        Err(error) => checks.push(missing(
            "reflink",
            json!({"supported": false, "detail": error}),
            "cp and stat are required to probe reflink support",
        )),
    }

    let idmap_entries = read_trimmed("/proc/self/mountinfo")
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains("idmap"))
        .count();
    checks.push(missing(
        "id_mapping",
        json!({"idmapped_mounts_observed": idmap_entries, "source": "/proc/self/mountinfo"}),
        "idmapped mounts are not configured; an idmap-capable kernel and storage backend are required",
    ));

    let clocksource = read_trimmed("/sys/devices/system/clocksource/clocksource0/current_clocksource");
    match clocksource {
        Some(value) if !value.is_empty() => checks.push(check(
            "clocks",
            Verdict::Pass,
            json!({"clocksource": value}),
            None,
        )),
        other => checks.push(missing(
            "clocks",
            json!({"clocksource": other}),
            "current clocksource is unavailable; /sys/devices/system/clocksource is required",
        )),
    }

    let limits = read_trimmed("/proc/self/limits").unwrap_or_default();
    let max_open_files = limits
        .lines()
        .find(|line| line.starts_with("Max open files"))
        .and_then(|line| line.split_whitespace().nth(3))
        .and_then(|value| value.parse::<u64>().ok());
    match max_open_files {
        Some(value) if value >= 1024 => checks.push(check(
            "fd_limits",
            Verdict::Pass,
            json!({"max_open_files": value}),
            None,
        )),
        other => checks.push(check(
            "fd_limits",
            match other {
                Some(_) => Verdict::Fail,
                None => Verdict::Missing,
            },
            json!({"max_open_files": other}),
            Some("file descriptor limit below 1024 or unreadable from /proc/self/limits".to_string()),
        )),
    }

    let runtime = config.smolvm_binary();
    let version = Command::new(&runtime).arg("--version").output();
    match version {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let compatible = text.starts_with("smolvm ") && text.ends_with(RUNTIME_VERSION);
            checks.push(check(
                "runtime_compatibility",
                if compatible { Verdict::Pass } else { Verdict::Fail },
                json!({"binary": runtime.to_string_lossy(), "version": text, "expected": RUNTIME_VERSION}),
                if compatible { None } else { Some("runtime version does not match the pinned version".to_string()) },
            ));
        }
        other => {
            let detail = match other {
                Ok(output) => String::from_utf8_lossy(&output.stderr).trim().to_string(),
                Err(error) => error.to_string(),
            };
            checks.push(missing(
                "runtime_compatibility",
                json!({"binary": runtime.to_string_lossy(), "detail": detail}),
                "the pinned smolvm runtime binary is not executable at the configured runtime directory",
            ));
        }
    }

    checks
}

pub fn doctor(config: &Config) -> Report {
    let mut checks = match Report::read(&config.state) {
        Ok(report) => report.checks,
        Err(_) => evaluate(&BoundaryInput::from_config(config)).checks,
    };
    checks.extend(extras(config));
    finish("doctor", checks)
}
