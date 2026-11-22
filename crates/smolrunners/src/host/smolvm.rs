use crate::host::config::Config;
use crate::host::environment;
use crate::host::marker::Marker;
use std::path::PathBuf;
use serde::Deserialize;
use std::process::{Child, Command, Output};

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Envelope {
    error: ErrorBody,
}

pub struct Smolvm<'a> {
    config: &'a Config,
    vm_dir: PathBuf,
    confinement: Option<Confinement>,
}

struct Confinement {
    uid: u32,
    cgroup: String,
    cpu_max: String,
    memory_max: String,
    pids_max: String,
}

impl<'a> Smolvm<'a> {
    pub fn unconfined(config: &'a Config, machine: &str) -> Self {
        Self {
            config,
            vm_dir: config.vm_dir(machine),
            confinement: None,
        }
    }

    pub fn for_marker(config: &'a Config, marker: &Marker) -> Self {
        Self {
            config,
            vm_dir: config.vm_dir(&marker.machine),
            confinement: Some(Confinement {
                uid: marker.uid,
                cgroup: marker.cgroup.clone(),
                cpu_max: marker.cpu_max.clone(),
                memory_max: marker.memory_max.clone(),
                pids_max: marker.pids_max.clone(),
            }),
        }
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = match &self.confinement {
            Some(confinement) => {
                let mut launcher = Command::new(&self.config.launcher);
                launcher
                    .arg("--uid")
                    .arg(confinement.uid.to_string())
                    .arg("--gid")
                    .arg(confinement.uid.to_string())
                    .arg("--cgroup")
                    .arg(&confinement.cgroup)
                    .arg("--cpu-max")
                    .arg(&confinement.cpu_max)
                    .arg("--memory-max")
                    .arg(&confinement.memory_max)
                    .arg("--pids-max")
                    .arg(&confinement.pids_max)
                    .arg("--net-allow")
                    .arg(&self.config.net_allow)
                    .arg("--")
                    .arg(self.config.smolvm_binary());
                launcher
            }
            None => Command::new(self.config.smolvm_binary()),
        };
        command.env_clear();
        for (name, value) in environment::sanitized(&self.vm_dir, &self.config.runtime) {
            command.env(name, value);
        }
        command
            .env("SMOLVM_SECCOMP", "enforce")
            .env("SMOLVM_LANDLOCK", "enforce")
            .arg("--error-format")
            .arg("json")
            .arg("--state-dir")
            .arg(self.vm_dir.join("smolvm"))
            .args(args);
        command
    }

    pub fn raw(&self, args: &[&str]) -> Result<Output, String> {
        self.command(args)
            .output()
            .map_err(|error| format!("spawn smolvm: {error}"))
    }

    pub fn invoke(&self, args: &[&str]) -> Result<Output, String> {
        let output = self.raw(args)?;
        if output.status.success() {
            return Ok(output);
        }
        let error = parse_failure(&output);
        Err(format!("{} failed: {} ({})", args.join(" "), error.message, error.code))
    }

    pub fn start_tracked(&self, args: &[&str]) -> Result<Running, String> {
        let child = self
            .command(args)
            .spawn()
            .map_err(|error| format!("spawn smolvm: {error}"))?;
        Ok(Running { child })
    }
}

pub struct Running {
    child: Child,
}

impl Running {
    pub fn wait(self) -> Result<Output, String> {
        self.child
            .wait_with_output()
            .map_err(|error| format!("wait for smolvm: {error}"))
    }
}

pub fn parse_failure(output: &Output) -> ErrorBody {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let trimmed = stderr.trim();
    if let Ok(envelope) = serde_json::from_str::<Envelope>(trimmed) {
        return envelope.error;
    }
    ErrorBody {
        code: "SMOLVM_ERROR".to_string(),
        message: trimmed.to_string(),
    }
}

pub fn named_machines(config: &Config) -> Result<Vec<String>, String> {
    let entries = match std::fs::read_dir(config.vms_dir()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", config.vms_dir().display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read vm dir entry: {error}"))?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}
