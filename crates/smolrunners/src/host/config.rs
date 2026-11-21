use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub state: PathBuf,
    pub runtime: PathBuf,
    pub launcher: PathBuf,
    pub uid: u32,
    pub cpu_max: String,
    pub memory_max: String,
    pub pids_max: String,
    pub net_allow: String,
    pub hold_after_create_ms: u64,
}

impl Config {
    pub fn net_mode(&self) -> String {
        let loopback = self
            .net_allow
            .split(',')
            .map(|entry| entry.trim().to_ascii_lowercase())
            .any(|entry| entry.starts_with("127.") || entry.starts_with("::1") || entry == "0.0.0.0/0" || entry == "::/0");
        if loopback {
            "allow-loopback".to_string()
        } else {
            "private".to_string()
        }
    }

    pub fn smolvm_binary(&self) -> PathBuf {
        self.runtime.join("smolvm")
    }

    pub fn smolvm_state(&self) -> PathBuf {
        self.state.join("smolvm")
    }

    pub fn control_file(&self, machine: &str) -> PathBuf {
        self.state.join("control").join(format!("stop-{machine}"))
    }
}
