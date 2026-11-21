use crate::host::config::Config;
use crate::host::marker::{self, Marker, Phase};
use crate::host::smolvm::{self, Smolvm};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
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
    match smolvm.raw(&["machine", "stop", "--name", &marker.machine], None) {
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
    match smolvm.raw(&["machine", "delete", "--name", &marker.machine, "-f"], None) {
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
    let mut marker = Marker {
        machine: machine.clone(),
        owner_pid: std::process::id(),
        incarnation: id,
        phase: Phase::Creating,
        image: image.to_string(),
        uid: config.uid,
        cgroup,
    };

    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;
    println!("CLAIM {}", marker.machine);
    let smolvm = Smolvm::new(config);
    smolvm.invoke(
        &[
            "machine",
            "create",
            "--name",
            &marker.machine,
            "-I",
            image,
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
    smolvm.invoke(&["machine", "start", "--name", &marker.machine], Some(&marker.cgroup))?;

    marker.phase = Phase::Started;
    marker::write(&config.state, &marker)
        .map_err(|error| format!("write marker: {error}"))?;
    println!("READY {}", marker.machine);

    wait_for_stop(config, &marker.machine);
    teardown(config, &marker)?;
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
