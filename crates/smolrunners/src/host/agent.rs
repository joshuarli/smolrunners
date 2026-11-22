use crate::host::boundary::{self, BoundaryInput, Report, Verdict};
use crate::host::config::Config;
use crate::host::marker::{self, Marker, Phase};
use crate::host::smolvm::{self, Smolvm};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, chown};
use std::path::{Path, PathBuf};
use std::process::Stdio;
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
        (config.state.clone(), 0o711),
        (config.vms_dir(), 0o711),
        (config.state.join("markers"), 0o700),
        (config.state.join("control"), 0o700),
        (config.state.join("logs"), 0o700),
        (config.state.join("uids"), 0o700),
    ];
    for (directory, mode) in directories {
        fs::create_dir_all(&directory)
            .map_err(|error| format!("create {}: {error}", directory.display()))?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(mode))
            .map_err(|error| format!("permit {}: {error}", directory.display()))?;
    }
    Ok(())
}

pub fn prepare_vm(config: &Config, machine: &str, owner: Option<u32>) -> Result<(), String> {
    let root = config.vm_dir(machine);
    for relative in ["", "smolvm", "home", "home/.config", "home/.cache", "home/.local/share", "tmp"] {
        let directory = if relative.is_empty() { root.clone() } else { root.join(relative) };
        fs::create_dir_all(&directory)
            .map_err(|error| format!("create {}: {error}", directory.display()))?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("permit {}: {error}", directory.display()))?;
        if let Some(uid) = owner {
            chown(&directory, Some(uid), Some(uid))
                .map_err(|error| format!("chown {}: {error}", directory.display()))?;
        }
    }
    Ok(())
}

fn retire(config: &Config, marker: &Marker) -> Result<(), String> {
    remove_cgroup(&marker.cgroup);
    remove_vm_dir(config, &marker.machine)?;
    if let Some(job) = &marker.storage_job {
        crate::host::storage::release(config, job)?;
    }
    release_uid(&config.state, marker.uid);
    Ok(())
}

fn remove_vm_dir(config: &Config, machine: &str) -> Result<(), String> {
    match fs::remove_dir_all(config.vm_dir(machine)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove vm dir {machine}: {error}")),
    }
}

fn remove_cgroup(path: &str) {
    if !path.starts_with("/sys/fs/cgroup/sr-") {
        return;
    }
    let _ = fs::remove_dir(path);
}

fn teardown(config: &Config, marker: &Marker) -> Result<(), String> {
    let smolvm = Smolvm::for_marker(config, marker);
    match smolvm.raw(&["machine", "stop", "--name", &marker.machine]) {
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
    match smolvm.raw(&["machine", "delete", "--name", &marker.machine, "-f"]) {
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
        retire(config, &marker)?;
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

fn stop_requested(config: &Config, machine: &str) -> bool {
    let control = config.control_file(machine);
    if control.exists() {
        let _ = fs::remove_file(&control);
        return true;
    }
    false
}

fn wait_for_stop(config: &Config, machine: &str) {
    while !stop_requested(config, machine) {
        thread::sleep(Duration::from_millis(100));
    }
}

pub struct RunnerLaunch {
    pub jit_file: PathBuf,
    pub entrypoint: String,
}

pub struct ProvisionRequest {
    pub role: String,
    pub image: String,
    pub cpus: u32,
    pub memory_mib: u32,
    pub storage_gib: u32,
    pub overlay_gib: u32,
    pub reservation_id: Option<u64>,
    pub execution_id: Option<i64>,
    pub runner: Option<RunnerLaunch>,
}

const GUEST_JIT_PATH: &str = "/run/smolrunners/jit";

fn take_jit(path: &Path) -> Result<String, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("read jit file: {error}"))?;
    fs::remove_file(path).map_err(|error| format!("remove jit file: {error}"))?;
    let trimmed = text.trim().to_owned();
    if trimmed.is_empty() {
        return Err("jit file is empty".to_owned());
    }
    Ok(trimmed)
}

fn deliver_jit(smolvm: &Smolvm, machine: &str, jit: &str) -> Result<(), String> {
    let mut child = smolvm
        .command(&[
            "machine", "exec", "--name", machine, "-i", "--", "sh", "-c",
            "umask 077 && mkdir -p /run/smolrunners && cat > /run/smolrunners/jit",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn jit delivery: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(jit.as_bytes())
            .map_err(|error| format!("write jit into guest: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for jit delivery: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "deliver jit config: {}",
            smolvm::parse_failure(&output).message
        ));
    }
    Ok(())
}

fn supervise_runner(config: &Config, marker: &Marker, entrypoint: &str, jit: &str) -> Result<(), String> {
    let smolvm = Smolvm::for_marker(config, marker);
    deliver_jit(&smolvm, &marker.machine, jit)?;
    println!("JIT_DELIVERED {}", marker.machine);
    let log_path = config.runner_log(&marker.machine);
    let log = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&log_path)
        .map_err(|error| format!("open {}: {error}", log_path.display()))?;
    let error_log = log.try_clone().map_err(|error| format!("clone runner log: {error}"))?;
    let mut runner = smolvm
        .command(&[
            "machine", "exec", "--name", &marker.machine, "--timeout", "6h", "--stream", "--",
            entrypoint, GUEST_JIT_PATH,
        ])
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(error_log)
        .spawn()
        .map_err(|error| format!("start runner: {error}"))?;
    println!("RUNNER_STARTED {}", marker.machine);
    loop {
        if let Some(status) = runner.try_wait().map_err(|error| format!("wait for runner: {error}"))? {
            match status.code() {
                Some(code) => println!("RUNNER_EXIT code={code}"),
                None => println!("RUNNER_EXIT signal"),
            }
            return Ok(());
        }
        if stop_requested(config, &marker.machine) {
            let _ = runner.kill();
            let _ = runner.wait();
            println!("RUNNER_EXIT canceled");
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn uid_claim(state: &Path, uid: u32) -> PathBuf {
    state.join("uids").join(uid.to_string())
}

fn claim_is_stale(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let mut words = text.split_whitespace();
    let pid = words.next().and_then(|word| word.parse::<u32>().ok());
    let start = words.next().and_then(|word| word.parse::<u64>().ok());
    match (pid, start) {
        (Some(pid), Some(start)) => !owner_matches(start, process_start(pid)),
        _ => false,
    }
}

fn allocate_uid(state: &Path) -> Result<u32, String> {
    const FIRST: u32 = 61_000;
    const COUNT: u32 = 1_000;
    let directory = state.join("uids");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    let owner = format!(
        "{} {}\n",
        std::process::id(),
        process_start(std::process::id()).unwrap_or(0)
    );
    for uid in FIRST..FIRST + COUNT {
        let path = uid_claim(state, uid);
        for _ in 0..2 {
            match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
                Ok(mut file) => {
                    file.write_all(owner.as_bytes())
                        .map_err(|error| format!("write uid claim {uid}: {error}"))?;
                    return Ok(uid);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if claim_is_stale(&path) {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    break;
                }
                Err(error) => return Err(format!("claim uid {uid}: {error}")),
            }
        }
    }
    Err("no per-job VM uid is available".to_owned())
}

fn release_uid(state: &Path, uid: u32) {
    let _ = fs::remove_file(uid_claim(state, uid));
}

pub fn provision(config: &Config, request: ProvisionRequest) -> Result<(), String> {
    let ProvisionRequest {
        role,
        image,
        cpus,
        memory_mib: memory,
        storage_gib: storage,
        overlay_gib: overlay,
        reservation_id,
        execution_id,
        runner,
    } = request;
    let role = role.as_str();
    let image = image.as_str();
    let runner = match runner {
        Some(launch) => Some((take_jit(&launch.jit_file)?, launch.entrypoint)),
        None => None,
    };
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

    let uid = match allocate_uid(&config.state) {
        Ok(uid) => uid,
        Err(error) => {
            let _ = crate::host::storage::release(config, &job_key);
            return Err(error);
        }
    };
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
        uid,
        cgroup,
        vmm_pid: None,
        net_allow: config.net_allow.clone(),
        net_mode: net_mode.clone(),
        reservation_id,
        execution_id,
        storage_gib: Some(storage),
        storage_job: Some(job_key.clone()),
        cpu_max: config.cpu_max.clone(),
        memory_max: config.memory_max.clone(),
        pids_max: config.pids_max.clone(),
    };

    prepare_vm(config, &marker.machine, Some(marker.uid))?;
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
    if runner.is_some() {
        create_args.extend_from_slice(&[
            "--net", "--net-backend", "virtio-net",
            "-v", "/var/lib/docker", "-v", "/var/lib/containerd",
        ]);
    }
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
    let smolvm = Smolvm::for_marker(config, &marker);
    smolvm.invoke(&create_args)?;

    marker.phase = Phase::Created;
    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;

    if config.hold_after_create_ms > 0 {
        thread::sleep(Duration::from_millis(config.hold_after_create_ms));
    }

    marker.phase = Phase::Starting;
    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;
    let running = smolvm.start_tracked(&["machine", "start", "--name", &marker.machine])?;

    let Some(vmm_pid) = wait_for_vmm(&marker.cgroup) else {
        let report = record_boundary(config, &marker)?;
        println!("INELIGIBLE {}", ineligible(&report));
        let _ = smolvm.invoke(&["machine", "stop", "--name", &marker.machine]);
        teardown(config, &marker)?;
        let _ = running.wait();
        retire(config, &marker)?;
        return Err("vmm did not become observable before readiness".to_string());
    };

    marker.vmm_pid = Some(vmm_pid);
    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;
    let report = record_boundary(config, &marker)?;
    if !report.eligible {
        println!("INELIGIBLE {}", ineligible(&report));
        let _ = smolvm.invoke(&["machine", "stop", "--name", &marker.machine]);
        teardown(config, &marker)?;
        let _ = running.wait();
        retire(config, &marker)?;
        return Err("host failed the boundary checks before readiness".to_string());
    }

    marker.phase = Phase::Started;
    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;
    println!("READY {}", marker.machine);

    let outcome = match &runner {
        Some((jit, entrypoint)) => supervise_runner(config, &marker, entrypoint, jit),
        None => {
            wait_for_stop(config, &marker.machine);
            Ok(())
        }
    };
    drop(runner);
    if let Err(error) = &outcome {
        println!("RUNNER_FAILED {error}");
    }
    teardown(config, &marker)?;
    let _ = running.wait();
    retire(config, &marker)?;
    println!("REMOVED {}", marker.machine);
    outcome
}

pub fn stop(config: &Config, machine: &str) -> Result<(), String> {
    let control: PathBuf = config.control_file(machine);
    fs::write(&control, b"stop")
        .map_err(|error| format!("write control {}: {error}", control.display()))
}

pub fn list(config: &Config, json: bool) -> Result<(), String> {
    let markers = marker::scan(&config.state)?;
    if json {
        let entries: Vec<serde_json::Value> = markers
            .iter()
            .map(|(_, marker)| {
                serde_json::json!({
                    "machine": marker.machine,
                    "phase": marker.phase,
                    "owner_alive": owner_alive(marker),
                    "reservation_id": marker.reservation_id,
                    "execution_id": marker.execution_id,
                    "cpu_max": marker.cpu_max,
                    "memory_max": marker.memory_max,
                    "pids_max": marker.pids_max,
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(entries));
        return Ok(());
    }
    for (_, marker) in &markers {
        println!(
            "{} phase={:?} owner={} alive={}",
            marker.machine,
            marker.phase,
            marker.owner_pid,
            owner_alive(marker)
        );
    }
    for machine in smolvm::named_machines(config)? {
        println!("MACHINE {machine}");
    }
    Ok(())
}

pub fn exec(config: &Config, machine: &str, flags: &[&str], command: &[String]) -> Result<i32, String> {
    let marker = marker::read(&marker::path(&config.state, machine))?;
    let smolvm = Smolvm::for_marker(config, &marker);
    let mut args: Vec<&str> = vec!["machine", "exec", "--name", machine];
    args.extend_from_slice(flags);
    args.push("--");
    args.extend(command.iter().map(String::as_str));
    let status = smolvm
        .command(&args)
        .status()
        .map_err(|error| format!("spawn exec: {error}"))?;
    Ok(status.code().unwrap_or(128))
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

    #[cfg(target_os = "linux")]
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
            storage_job: None,
            cpu_max: String::new(),
            memory_max: String::new(),
            pids_max: String::new(),
        }
    }

    fn config(state: &Path) -> Config {
        Config {
            state: state.to_path_buf(),
            runtime: PathBuf::from("/nonexistent-runtime"),
            launcher: PathBuf::from("/nonexistent-launcher"),
            uid: 60_000,
            cpu_max: String::new(),
            memory_max: String::new(),
            pids_max: String::new(),
            net_allow: "10.0.0.0/8".to_owned(),
            hold_after_create_ms: 0,
        }
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
    }

    #[test]
    fn shared_authority_state_is_private_and_vm_state_is_per_machine() {
        let dir = TestDir::new();
        let config = config(&dir.0);
        prepare_state(&config).expect("prepare state");
        prepare_vm(&config, "sr-a-job", None).expect("prepare vm");
        assert_eq!(mode(&dir.0), 0o711);
        assert_eq!(mode(&dir.0.join("vms")), 0o711);
        for private in ["markers", "control", "logs"] {
            assert_eq!(mode(&dir.0.join(private)), 0o700, "{private}");
        }
        for relative in ["", "smolvm", "home", "tmp"] {
            assert_eq!(mode(&config.vm_dir("sr-a-job").join(relative)), 0o700, "vm {relative}");
        }
        assert!(!dir.0.join("smolvm").exists(), "no state is shared between VM uids");
        remove_vm_dir(&config, "sr-a-job").expect("remove vm dir");
        assert!(!config.vm_dir("sr-a-job").exists());
    }

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn concurrent_allocations_never_share_a_uid() {
        let dir = TestDir::new();
        let state = dir.0.clone();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let state = state.clone();
                std::thread::spawn(move || allocate_uid(&state).expect("uid"))
            })
            .collect();
        let mut uids: Vec<u32> = handles.into_iter().map(|handle| handle.join().expect("join")).collect();
        uids.sort();
        uids.dedup();
        assert_eq!(uids.len(), 8, "every concurrent provision gets its own uid");
        release_uid(&state, uids[0]);
        assert_eq!(allocate_uid(&state).expect("reuse"), uids[0], "a released uid is reusable");
    }

    #[test]
    fn a_reused_pid_is_not_the_recorded_owner() {
        assert!(owner_matches(42, Some(42)));
        assert!(owner_matches(0, Some(42)), "legacy markers without a start time still match");
        assert!(!owner_matches(41, Some(42)));
        assert!(!owner_matches(42, None));
    }
}
