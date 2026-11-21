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
