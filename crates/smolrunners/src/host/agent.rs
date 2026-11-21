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

fn owner_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
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
    match smolvm.raw(&["machine", "stop", "--name", &marker.machine], Some(&marker.cgroup)) {
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
    match smolvm.raw(
        &["machine", "delete", "--name", &marker.machine, "-f"],
        Some(&marker.cgroup),
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
        if owner_alive(marker.owner_pid) {
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
        uid: config.uid,
        cpu_max: config.cpu_max.clone(),
        memory_max: config.memory_max.clone(),
        pids_max: config.pids_max.clone(),
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

pub fn provision(
    config: &Config,
    role: &str,
    image: &str,
    cpus: u32,
    memory: u32,
    storage: u32,
    overlay: u32,
) -> Result<(), String> {
    prepare_state(config)?;
    for machine in reap(config)? {
        println!("REAPED {machine}");
    }

    let id = incarnation();
    let machine = machine_name(&id, role);
    let cgroup = format!("/sys/fs/cgroup/{machine}");
    let net_mode = config.net_mode();
    let mut marker = Marker {
        machine,
        owner_pid: std::process::id(),
        incarnation: id,
        phase: Phase::Creating,
        image: image.to_string(),
        uid: config.uid,
        cgroup,
        vmm_pid: None,
        net_allow: config.net_allow.clone(),
        net_mode: net_mode.clone(),
    };

    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;
    println!("CLAIM {}", marker.machine);
    let network_flag = if net_mode == "private" {
        "--block-private"
    } else {
        "--allow-host-loopback"
    };
    let smolvm = Smolvm::new(config);
    smolvm.invoke(
        &[
            "machine",
            "create",
            "--name",
            &marker.machine,
            "-I",
            image,
            network_flag,
            "--cpus",
            &cpus.to_string(),
            "--mem",
            &memory.to_string(),
            "--storage",
            &storage.to_string(),
            "--overlay",
            &overlay.to_string(),
        ],
        Some(&marker.cgroup),
    )?;

    marker.phase = Phase::Created;
    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;

    if config.hold_after_create_ms > 0 {
        thread::sleep(Duration::from_millis(config.hold_after_create_ms));
    }

    marker.phase = Phase::Starting;
    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;
    let running = smolvm.start_tracked(&["machine", "start", "--name", &marker.machine], Some(&marker.cgroup))?;

    let Some(vmm_pid) = wait_for_vmm(&marker.cgroup) else {
        let report = record_boundary(config, &marker)?;
        println!("INELIGIBLE {}", ineligible(&report));
        let _ = smolvm.invoke(&["machine", "stop", "--name", &marker.machine], Some(&marker.cgroup));
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
        let _ = smolvm.invoke(&["machine", "stop", "--name", &marker.machine], Some(&marker.cgroup));
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
            owner_alive(marker.owner_pid)
        );
        let _ = path;
    }
    for machine in smolvm::named_machines(config)? {
        println!("MACHINE {machine}");
    }
    Ok(())
}
