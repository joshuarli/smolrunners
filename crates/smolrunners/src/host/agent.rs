use crate::host::boundary::{self, BoundaryInput, Report, Verdict};
use crate::host::config::Config;
use crate::host::marker::{self, Marker, Phase};
use crate::host::smolvm::{self, Smolvm};
use std::fs;
use std::io;
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn process_start(pid: u32) -> Option<u64> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = text.rsplit_once(')')?.1.split_whitespace().collect::<Vec<_>>();
    tail.get(19)?.parse::<u64>().ok()
}

fn owner_matches(recorded: u64, observed: Option<u64>) -> bool {
    match observed {
        None => false,
        Some(start) => recorded == 0 || recorded == start,
    }
}

fn owner_alive(marker: &Marker) -> bool {
    owner_matches(marker.owner_start, process_start(marker.owner_pid))
}

fn incarnation() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", std::process::id(), nanos)
}

fn machine_name(incarnation: &str, role: &str) -> String {
    let suffix = &incarnation[incarnation.len().saturating_sub(8)..];
    format!("sr-{suffix}-{role}")
}

pub fn prepare_state(config: &Config) -> Result<(), String> {
    let directories = [
        config.state.clone(),
        config.smolvm_state(),
        config.state.join("home"),
        config.state.join("home/.config"),
        config.state.join("home/.cache"),
        config.state.join("home/.local/share"),
        config.state.join("tmp"),
        config.state.join("markers"),
        config.state.join("control"),
    ];
    for directory in directories {
        fs::create_dir_all(&directory)
            .map_err(|error| format!("create {}: {error}", directory.display()))?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o777))
            .map_err(|error| format!("permit {}: {error}", directory.display()))?;
        chown(&directory, Some(config.uid), Some(config.uid))
            .map_err(|error| format!("chown {}: {error}", directory.display()))?;
    }
    Ok(())
}

fn remove_cgroup(path: &str) {
    if !path.starts_with("/sys/fs/cgroup/sr-") {
        return;
    }
    let _ = fs::remove_dir(path);
}

fn teardown(config: &Config, marker: &Marker) -> Result<(), String> {
    let smolvm = Smolvm::new(config);
    match smolvm.raw_as(&["machine", "stop", "--name", &marker.machine], Some(&marker.cgroup), marker.uid) {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let error = smolvm::parse_failure(&output);
            if error.code != "NOT_FOUND" && error.code != "INVALID_STATE" {
                return Err(format!(
                    "stop {}: {} ({})",
                    marker.machine, error.message, error.code
                ));
            }
        }
        Err(error) => return Err(error),
    }
    match smolvm.raw_as(
        &["machine", "delete", "--name", &marker.machine, "-f"],
        Some(&marker.cgroup),
        marker.uid,
    ) {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let error = smolvm::parse_failure(&output);
            if error.code != "NOT_FOUND" {
                return Err(format!(
                    "delete {}: {} ({})",
                    marker.machine, error.message, error.code
                ));
            }
        }
        Err(error) => return Err(error),
    }
    remove_cgroup(&marker.cgroup);
    marker::remove(&config.state, &marker.machine)
        .map_err(|error| format!("remove marker {}: {error}", marker.machine))
}

pub fn reap(config: &Config) -> Result<Vec<String>, String> {
    let mut reaped = Vec::new();
    for (path, marker) in marker::scan(&config.state)? {
        if owner_alive(&marker) {
            continue;
        }
        teardown(config, &marker)?;
        if let Err(error) = fs::remove_file(&path) {
            if error.kind() != io::ErrorKind::NotFound {
                return Err(format!("remove {}: {error}", path.display()));
            }
        }
        reaped.push(marker.machine);
    }
    Ok(reaped)
}

fn wait_for_vmm(cgroup: &str) -> Option<u32> {
    let procs = format!("{cgroup}/cgroup.procs");
    for _ in 0..450 {
        if let Ok(bytes) = fs::read(&procs) {
            for word in String::from_utf8_lossy(&bytes).split_whitespace() {
                if let Ok(pid) = word.parse::<u32>() {
                    if let Ok(status) = fs::read(format!("/proc/{pid}/status")) {
                        let text = String::from_utf8_lossy(&status);
                        let is_vmm = text.lines().any(|line| {
                            line.strip_prefix("Name:").is_some_and(|value| value.trim() == "libkrun VM")
                        });
                        if is_vmm && text.contains("\nSeccomp:") && text.contains("\nNoNewPrivs:") {
                            return Some(pid);
                        }
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    None
}

fn boundary_input(config: &Config, marker: &Marker) -> BoundaryInput {
    BoundaryInput {
        uid: marker.uid,
        cpu_max: if marker.cpu_max.is_empty() { config.cpu_max.clone() } else { marker.cpu_max.clone() },
        memory_max: if marker.memory_max.is_empty() { config.memory_max.clone() } else { marker.memory_max.clone() },
        pids_max: if marker.pids_max.is_empty() { config.pids_max.clone() } else { marker.pids_max.clone() },
        net_allow: config.net_allow.clone(),
        net_mode: config.net_mode(),
        machine: marker.machine.clone(),
        cgroup: marker.cgroup.clone(),
        incarnation: marker.incarnation.clone(),
        vmm_pid: marker.vmm_pid,
        policy_variables: boundary::host_policy_variables(),
    }
}

fn record_boundary(config: &Config, marker: &Marker) -> Result<Report, String> {
    let report = boundary::evaluate(&boundary_input(config, marker));
    report.write(&config.state)?;
    Ok(report)
}

fn ineligible(report: &Report) -> String {
    report
        .checks
        .iter()
        .filter(|item| item.verdict == Verdict::Fail || item.verdict == Verdict::Missing)
        .map(|item| format!("{}: {}", item.name, item.reason.clone().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("; ")
}

fn wait_for_stop(config: &Config, machine: &str) {
    let control = config.control_file(machine);
    while !control.exists() {
        thread::sleep(Duration::from_millis(100));
    }
    let _ = fs::remove_file(&control);
}

fn allocate_uid(state: &Path) -> Result<u32, String> {
    const FIRST: u32 = 61_000;
    const COUNT: u32 = 1_000;
    let used: std::collections::BTreeSet<u32> = marker::scan(state)?
        .into_iter()
        .map(|(_, marker)| marker.uid)
        .collect();
    (0..COUNT)
        .map(|offset| FIRST + offset)
        .find(|uid| !used.contains(uid))
        .ok_or_else(|| "no per-job VM uid is available".to_owned())
}

pub fn provision(
    config: &Config,
    role: &str,
    image: &str,
    cpus: u32,
    memory: u32,
    storage: u32,
    overlay: u32,
    reservation_id: Option<u64>,
    execution_id: Option<i64>,
) -> Result<(), String> {
    prepare_state(config)?;
    for machine in reap(config)? {
        println!("REAPED {machine}");
    }

    let job_key = execution_id
        .map(|value| format!("exec-{value}"))
        .unwrap_or_else(|| role.to_owned());
    let storage_bytes = u64::from(storage).saturating_mul(1024 * 1024 * 1024);
    let admission = crate::host::storage::admit(config, &job_key, storage_bytes, 1_024)?;
    if !admission.admitted {
        return Err(format!("storage admission refused: {}", admission.reason.unwrap_or_default()));
    }
    println!(
        "ADMITTED job={job_key} bytes={} charge={}",
        storage_bytes, admission.charge_bytes
    );

    let id = incarnation();
    let machine = machine_name(&id, role);
    let cgroup = format!("/sys/fs/cgroup/{machine}");
    let net_mode = config.net_mode();
    let mut marker = Marker {
        machine,
        owner_pid: std::process::id(),
        owner_start: process_start(std::process::id()).unwrap_or(0),
        incarnation: id,
        phase: Phase::Creating,
        image: image.to_string(),
        uid: allocate_uid(&config.state)?,
        cgroup,
        vmm_pid: None,
        net_allow: config.net_allow.clone(),
        net_mode: net_mode.clone(),
        reservation_id,
        execution_id,
        storage_gib: Some(storage),
        cpu_max: config.cpu_max.clone(),
        memory_max: config.memory_max.clone(),
        pids_max: config.pids_max.clone(),
    };

    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;
    println!("CLAIM {}", marker.machine);
    let cpus_arg = cpus.to_string();
    let memory_arg = memory.to_string();
    let storage_arg = storage.to_string();
    let overlay_arg = overlay.to_string();
    let mut create_args: Vec<&str> = vec![
        "machine",
        "create",
        "--name",
        &marker.machine,
        "-I",
        image,
    ];
    if net_mode == "private" {
        create_args.push("--block-private");
    }
    create_args.extend_from_slice(&[
        "--cpus",
        &cpus_arg,
        "--mem",
        &memory_arg,
        "--storage",
        &storage_arg,
        "--overlay",
        &overlay_arg,
    ]);
    let smolvm = Smolvm::new(config);
    smolvm.invoke_as(&create_args, Some(&marker.cgroup), marker.uid)?;

    marker.phase = Phase::Created;
    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;

    if config.hold_after_create_ms > 0 {
        thread::sleep(Duration::from_millis(config.hold_after_create_ms));
    }

    marker.phase = Phase::Starting;
    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;
    let running = smolvm.start_tracked_as(&["machine", "start", "--name", &marker.machine], Some(&marker.cgroup), marker.uid)?;

    let Some(vmm_pid) = wait_for_vmm(&marker.cgroup) else {
        let report = record_boundary(config, &marker)?;
        println!("INELIGIBLE {}", ineligible(&report));
        let _ = smolvm.invoke_as(&["machine", "stop", "--name", &marker.machine], Some(&marker.cgroup), marker.uid);
        drop(running);
        teardown(config, &marker)?;
        return Err("vmm did not become observable before readiness".to_string());
    };

    marker.vmm_pid = Some(vmm_pid);
    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;
    let report = record_boundary(config, &marker)?;
    if !report.eligible {
        println!("INELIGIBLE {}", ineligible(&report));
        let _ = smolvm.invoke_as(&["machine", "stop", "--name", &marker.machine], Some(&marker.cgroup), marker.uid);
        drop(running);
        teardown(config, &marker)?;
        return Err("host failed the boundary checks before readiness".to_string());
    }

    marker.phase = Phase::Started;
    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;
    println!("READY {}", marker.machine);

    wait_for_stop(config, &marker.machine);
    teardown(config, &marker)?;
    let _ = running.wait();
    println!("REMOVED {}", marker.machine);
    Ok(())
}

pub fn stop(config: &Config, machine: &str) -> Result<(), String> {
    let control: PathBuf = config.control_file(machine);
    fs::write(&control, b"stop")
        .map_err(|error| format!("write control {}: {error}", control.display()))
}

pub fn list(config: &Config) -> Result<(), String> {
    for (path, marker) in marker::scan(&config.state)? {
        println!(
            "{} phase={:?} owner={} alive={}",
            marker.machine,
            marker.phase,
            marker.owner_pid,
            owner_alive(&marker)
        );
        let _ = path;
    }
    for machine in smolvm::named_machines(config)? {
        println!("MACHINE {machine}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let sequence = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "smolrunners-uid-{}-{sequence}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp state dir");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn marker(machine: &str, uid: u32) -> Marker {
        Marker {
            machine: machine.to_owned(),
            owner_pid: std::process::id(),
            owner_start: process_start(std::process::id()).unwrap_or(0),
            incarnation: "i".to_owned(),
            phase: Phase::Creating,
            image: "image".to_owned(),
            uid,
            cgroup: format!("/sys/fs/cgroup/{machine}"),
            vmm_pid: None,
            net_allow: "10.0.0.0/8".to_owned(),
            net_mode: "private".to_owned(),
            reservation_id: None,
            execution_id: None,
            storage_gib: None,
            cpu_max: String::new(),
            memory_max: String::new(),
            pids_max: String::new(),
        }
    }

    #[test]
    fn each_job_gets_a_distinct_uid() {
        let dir = TestDir::new();
        let first = allocate_uid(&dir.0).expect("first uid");
        marker::write(&dir.0, &marker("machine-a", first)).expect("write marker a");
        let second = allocate_uid(&dir.0).expect("second uid");
        assert_ne!(first, second);
        marker::write(&dir.0, &marker("machine-b", second)).expect("write marker b");
        let third = allocate_uid(&dir.0).expect("third uid");
        assert_ne!(third, first);
        assert_ne!(third, second);
    }

    #[test]
    fn a_reused_pid_is_not_the_recorded_owner() {
        let mut owned = marker("machine-a", 61_000);
        assert!(owner_matches(42, Some(42)));
        assert!(owner_matches(0, Some(42)), "legacy markers without a start time still match");
        assert!(!owner_matches(41, Some(42)));
        assert!(!owner_matches(42, None));
    }
}
