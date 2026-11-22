use std::collections::HashMap;

use crate::domain::Shape;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CacheState {
    Cold,
    Warm,
}

impl CacheState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cold => "cold",
            Self::Warm => "warm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Executor {
    OfficialRunner,
    Fixture,
}

impl Executor {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OfficialRunner => "official-runner",
            Self::Fixture => "fixture",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComparableRunKey {
    pub repository: String,
    pub workflow: String,
    pub job: String,
    pub matrix: String,
    pub image: String,
    pub executor: Executor,
    pub profile_revision: String,
    pub hardware_class: String,
    pub cache_state: CacheState,
}

impl ComparableRunKey {
    pub fn new(
        repository: impl Into<String>,
        workflow: impl Into<String>,
        job: impl Into<String>,
        matrix: impl Into<String>,
        image: impl Into<String>,
        executor: Executor,
        profile_revision: impl Into<String>,
        hardware_class: impl Into<String>,
        cache_state: CacheState,
    ) -> Self {
        Self {
            repository: repository.into(),
            workflow: workflow.into(),
            job: job.into(),
            matrix: matrix.into(),
            image: image.into(),
            executor,
            profile_revision: profile_revision.into(),
            hardware_class: hardware_class.into(),
            cache_state,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failed,
    Oom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OomClass {
    None,
    WorkloadCap,
    ServiceCap,
    HostOom { explained: bool },
    Unknown,
}

impl OomClass {
    pub const fn is_oom(self) -> bool {
        !matches!(self, Self::None)
    }

    pub const fn is_vm_shortage(self) -> bool {
        matches!(self, Self::WorkloadCap)
    }

    pub const fn blocks_conclusion(self) -> bool {
        matches!(
            self,
            Self::HostOom { explained: false } | Self::Unknown
        )
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::WorkloadCap => "workload-cap",
            Self::ServiceCap => "service-cap",
            Self::HostOom { explained: true } => "host-explained",
            Self::HostOom { explained: false } => "host-unexplained",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Censor {
    None,
    HardLimit,
    Oom,
}

impl Censor {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::HardLimit => "hard-limit",
            Self::Oom => "oom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Integrals {
    pub allocated_cpu_core_seconds: f64,
    pub consumed_cpu_core_seconds: f64,
    pub allocated_memory_gib_seconds: f64,
    pub consumed_memory_gib_seconds: f64,
}

impl Integrals {
    pub fn cpu_utilization(self) -> f64 {
        self.consumed_cpu_core_seconds / self.allocated_cpu_core_seconds
    }

    pub fn memory_utilization(self) -> f64 {
        self.consumed_memory_gib_seconds / self.allocated_memory_gib_seconds
    }
}

#[derive(Debug, Clone)]
pub struct RunRecord {
    pub key: ComparableRunKey,
    pub outcome: Outcome,
    pub oom: OomClass,
    pub censor: Censor,
    pub integrals: Integrals,
    pub peak_cpu_cores: f64,
    pub peak_memory_gib: f64,
    pub shape: Shape,
    pub wall_seconds: f64,
    pub sample_count: u32,
    pub coverage_fraction: f64,
    pub gap_count: u32,
    pub definition_id: String,
    pub cohort: i64,
}

impl RunRecord {
    pub fn is_censored(&self) -> bool {
        self.censor != Censor::None || self.oom.is_oom()
    }

    pub fn mean_cpu_cores(&self) -> f64 {
        self.integrals.consumed_cpu_core_seconds / self.wall_seconds
    }

    pub fn mean_memory_gib(&self) -> f64 {
        self.integrals.consumed_memory_gib_seconds / self.wall_seconds
    }

    pub fn has_missing_coverage(&self) -> bool {
        self.gap_count > 0
            || self.coverage_fraction < super::recommend::MIN_COVERAGE
            || self.sample_count < super::recommend::MIN_SAMPLES
    }
}

pub fn group_runs(runs: impl IntoIterator<Item = RunRecord>) -> HashMap<ComparableRunKey, Vec<RunRecord>> {
    let mut groups: HashMap<ComparableRunKey, Vec<RunRecord>> = HashMap::new();
    for run in runs {
        groups.entry(run.key.clone()).or_default().push(run);
    }
    groups
}
