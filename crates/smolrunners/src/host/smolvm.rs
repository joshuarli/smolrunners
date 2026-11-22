use crate::host::config::Config;
use crate::host::environment;
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
}

impl<'a> Smolvm<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self { config }
    }

    fn command(&self, args: &[String], cgroup: Option<&str>) -> Command {
        self.command_as(args, cgroup, self.config.uid)
    }

    fn command_as(&self, args: &[String], cgroup: Option<&str>, uid: u32) -> Command {
        let mut command = match cgroup {
            Some(cgroup) => {
                let mut launcher = Command::new(&self.config.launcher);
                launcher
                    .arg("--uid")
                    .arg(uid.to_string())
                    .arg("--gid")
                    .arg(uid.to_string())
                    .arg("--cgroup")
                    .arg(cgroup)
                    .arg("--cpu-max")
                    .arg(&self.config.cpu_max)
                    .arg("--memory-max")
                    .arg(&self.config.memory_max)
                    .arg("--pids-max")
                    .arg(&self.config.pids_max)
                    .arg("--net-allow")
                    .arg(&self.config.net_allow)
                    .arg("--")
                    .arg(self.config.smolvm_binary());
                launcher
            }
            None => Command::new(self.config.smolvm_binary()),
        };
        command.env_clear();
        for (name, value) in environment::sanitized(&self.config.state, &self.config.runtime) {
            command.env(name, value);
        }
        command
            .env("SMOLVM_SECCOMP", "enforce")
            .env("SMOLVM_LANDLOCK", "enforce")
            .arg("--error-format")
            .arg("json")
            .arg("--state-dir")
            .arg(self.config.smolvm_state())
            .args(args);
        command
    }

    pub fn raw(&self, args: &[&str], cgroup: Option<&str>) -> Result<Output, String> {
        let owned: Vec<String> = args.iter().map(|value| (*value).to_string()).collect();
        self.command(&owned, cgroup)
            .output()
            .map_err(|error| format!("spawn smolvm: {error}"))
    }

    pub fn invoke(&self, args: &[&str], cgroup: Option<&str>) -> Result<Output, String> {
        self.invoke_as(args, cgroup, self.config.uid)
    }

    pub fn raw_as(&self, args: &[&str], cgroup: Option<&str>, uid: u32) -> Result<Output, String> {
        let owned: Vec<String> = args.iter().map(|value| (*value).to_string()).collect();
        self.command_as(&owned, cgroup, uid)
            .output()
            .map_err(|error| format!("spawn smolvm: {error}"))
    }

    pub fn invoke_as(&self, args: &[&str], cgroup: Option<&str>, uid: u32) -> Result<Output, String> {
        let output = self.raw_as(args, cgroup, uid)?;
        if output.status.success() {
            return Ok(output);
        }
        let error = parse_failure(&output);
        Err(format!("{} failed: {} ({})", args.join(" "), error.message, error.code))
    }

    pub fn start_tracked(&self, args: &[&str], cgroup: Option<&str>) -> Result<Running, String> {
        self.start_tracked_as(args, cgroup, self.config.uid)
    }

    pub fn start_tracked_as(
        &self,
        args: &[&str],
        cgroup: Option<&str>,
        uid: u32,
    ) -> Result<Running, String> {
        let owned: Vec<String> = args.iter().map(|value| (*value).to_string()).collect();
        let child = self
            .command_as(&owned, cgroup, uid)
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
    let smolvm = Smolvm::new(config);
    let output = smolvm.invoke(&["machine", "ls", "--json"], None)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed.starts_with("No machines") {
        return Ok(Vec::new());
    }
    let values: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|error| format!("parse machine ls: {error}"))?;
    let mut names = Vec::new();
    if let Some(array) = values.as_array() {
        for entry in array {
            if let Some(name) = entry.get("name").and_then(|value| value.as_str()) {
                names.push(name.to_string());
            }
        }
    }
    Ok(names)
}
