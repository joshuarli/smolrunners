use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use smolrunners_core::domain::Shape;
use smolrunners_core::ledger::{HostAllowance, VmmOverhead};

use crate::api::EffectiveLimits;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PilotConfig {
    pub(crate) state_dir: PathBuf,
    pub(crate) listen: SocketAddr,
    pub(crate) max_concurrency: u32,
    #[serde(default = "default_idle_timeout_secs")]
    pub(crate) idle_timeout_secs: u64,
    #[serde(default = "default_completion_grace_secs")]
    pub(crate) completion_grace_secs: u64,
    pub(crate) adapter: AdapterConfig,
    pub(crate) host: HostConfig,
    pub(crate) profile: ProfileConfig,
    pub(crate) vmm_overhead: OverheadConfig,
    #[serde(default)]
    pub(crate) telemetry: TelemetryConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum AdapterConfig {
    Github { program: PathBuf, github_config: PathBuf },
    Fake { program: PathBuf, scenario: String, scenarios_dir: PathBuf },
}

impl AdapterConfig {
    pub(crate) fn program(&self) -> &Path {
        match self {
            Self::Github { program, .. } | Self::Fake { program, .. } => program,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostConfig {
    pub(crate) key: String,
    pub(crate) arch: String,
    pub(crate) vcpu: u32,
    pub(crate) memory_gib: u32,
    pub(crate) disk_gib: u64,
    pub(crate) agent: PathBuf,
    pub(crate) launcher: PathBuf,
    pub(crate) runtime_dir: PathBuf,
    pub(crate) state_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileConfig {
    pub(crate) name: String,
    pub(crate) labels: Vec<String>,
    pub(crate) vcpu: u16,
    pub(crate) memory_gib: u32,
    pub(crate) storage_gib: u32,
    pub(crate) overlay_gib: u32,
    pub(crate) image: String,
    pub(crate) entrypoint: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OverheadConfig {
    pub(crate) memory_mib: u32,
    pub(crate) vcpu_milli: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TelemetryConfig {
    pub(crate) collector: String,
    pub(crate) interval_ms: u64,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            collector: "/opt/smolrunners/bin/smolrunners-collector".to_owned(),
            interval_ms: 1000,
        }
    }
}

fn default_idle_timeout_secs() -> u64 {
    900
}

fn default_completion_grace_secs() -> u64 {
    120
}

impl PilotConfig {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
        let config: Self =
            toml::from_str(&text).map_err(|error| format!("parse {}: {error}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        if !self.listen.ip().is_loopback() {
            return Err(format!(
                "listen {} is not a loopback address; the pilot API has no operator authentication",
                self.listen
            ));
        }
        if self.max_concurrency == 0 {
            return Err("max_concurrency must be at least 1".to_owned());
        }
        if self.profile.labels.is_empty() {
            return Err("profile.labels must name the scale set labels".to_owned());
        }
        let shape = self.shape()?;
        let allowance = self.allowance()?;
        let needed_milli = u64::from(self.max_concurrency) * (u64::from(shape.vcpu().get()) * 1000 + u64::from(self.vmm_overhead.vcpu_milli));
        if needed_milli > u64::from(self.host.vcpu) * 1000 {
            return Err(format!(
                "host.vcpu {} cannot hold {} guests of {} vCPU plus overhead",
                self.host.vcpu, self.max_concurrency, shape.vcpu().get()
            ));
        }
        let needed_bytes = u64::from(self.max_concurrency) * allowance.memory_bytes;
        if needed_bytes > u64::from(self.host.memory_gib) * 1024 * 1024 * 1024 {
            return Err(format!(
                "host.memory_gib {} cannot hold {} guests of {} GiB plus overhead",
                self.host.memory_gib, self.max_concurrency, shape.memory_gib()
            ));
        }
        Ok(())
    }

    pub(crate) fn shape(&self) -> Result<Shape, String> {
        Shape::new(self.profile.vcpu, self.profile.memory_gib).map_err(|error| error.to_string())
    }

    pub(crate) fn overhead(&self) -> VmmOverhead {
        VmmOverhead::new(self.vmm_overhead.memory_mib, self.vmm_overhead.vcpu_milli)
    }

    pub(crate) fn allowance(&self) -> Result<HostAllowance, String> {
        Ok(HostAllowance::for_shape(self.shape()?, self.overhead()))
    }

    pub(crate) fn limits(&self) -> Result<EffectiveLimits, String> {
        let allowance = self.allowance()?;
        Ok(EffectiveLimits {
            guest_vcpu: self.profile.vcpu,
            guest_memory_gib: self.profile.memory_gib,
            storage_gib: self.profile.storage_gib,
            host_cpu_max: allowance.cpu_max(),
            host_memory_max_bytes: allowance.memory_bytes,
            host_pids_max: allowance.pids,
        })
    }

    pub(crate) fn runner_prefix(&self) -> String {
        format!("smol-{}", self.host.key)
    }
}
