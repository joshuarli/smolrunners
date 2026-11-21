use std::collections::HashMap;

use sha2::{Digest, Sha256};

use super::schema::{Scope, Unit, SCHEMA_VERSION};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MetricDefinition {
    pub scope: Scope,
    pub name: String,
    pub unit: Unit,
}

fn metric(scope: Scope, name: &str, unit: Unit) -> MetricDefinition {
    MetricDefinition {
        scope,
        name: name.to_owned(),
        unit,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SampleDefinition {
    pub schema_version: u32,
    pub metrics: Vec<MetricDefinition>,
}

impl SampleDefinition {
    pub fn v1() -> Self {
        use Scope::{Guest, Host, Workload};
        use Unit::{Bytes, Count, Cores, Microseconds, Percent};
        Self {
            schema_version: SCHEMA_VERSION,
            metrics: vec![
                metric(Guest, "cpu.busy_cores", Cores),
                metric(Guest, "cpu.utilization_percent", Percent),
                metric(Guest, "cpu.busy_usec_total", Microseconds),
                metric(Guest, "cpu.total_usec_total", Microseconds),
                metric(Guest, "cpu.iowait_usec_total", Microseconds),
                metric(Guest, "cpu.steal_jiffies", Count),
                metric(Guest, "mem.total_bytes", Bytes),
                metric(Guest, "mem.available_bytes", Bytes),
                metric(Guest, "mem.used_bytes", Bytes),
                metric(Guest, "mem.cached_bytes", Bytes),
                metric(Guest, "mem.buffers_bytes", Bytes),
                metric(Guest, "mem.shmem_bytes", Bytes),
                metric(Guest, "mem.swap_total_bytes", Bytes),
                metric(Guest, "mem.swap_used_bytes", Bytes),
                metric(Guest, "psi.cpu.some_total", Count),
                metric(Guest, "psi.memory.some_total", Count),
                metric(Guest, "psi.io.some_total", Count),
                metric(Guest, "psi.cpu.some_avg10", Percent),
                metric(Guest, "psi.memory.some_avg10", Percent),
                metric(Guest, "disk.read_bytes_total", Bytes),
                metric(Guest, "disk.write_bytes_total", Bytes),
                metric(Guest, "disk.read_bytes_delta", Bytes),
                metric(Guest, "disk.write_bytes_delta", Bytes),
                metric(Guest, "net.rx_bytes_total", Bytes),
                metric(Guest, "net.tx_bytes_total", Bytes),
                metric(Guest, "net.rx_bytes_delta", Bytes),
                metric(Guest, "net.tx_bytes_delta", Bytes),
                metric(Workload, "cgroup.cpu.usage_usec", Microseconds),
                metric(Workload, "cgroup.cpu.throttled_usec", Microseconds),
                metric(Workload, "cgroup.cpu.nr_throttled", Count),
                metric(Workload, "cgroup.memory.current", Bytes),
                metric(Workload, "cgroup.memory.peak", Bytes),
                metric(Workload, "cgroup.memory.max", Bytes),
                metric(Workload, "cgroup.memory.swap.current", Bytes),
                metric(Workload, "cgroup.memory.events.oom", Count),
                metric(Workload, "cgroup.memory.events.oom_kill", Count),
                metric(Workload, "cgroup.memory.events.max", Count),
                metric(Workload, "cgroup.memory.anon", Bytes),
                metric(Workload, "cgroup.memory.file", Bytes),
                metric(Workload, "cgroup.io.read_bytes", Bytes),
                metric(Workload, "cgroup.io.write_bytes", Bytes),
                metric(Workload, "cgroup.descendants", Count),
                metric(Host, "host.cgroup.cpu.usage_usec", Microseconds),
                metric(Host, "host.cgroup.memory.current", Bytes),
                metric(Host, "host.cpu.steal_jiffies", Count),
                metric(Host, "host.pressure.cpu.some_total", Count),
                metric(Host, "host.pressure.memory.some_total", Count),
            ],
        }
    }

    pub fn canonical(&self) -> String {
        let mut entries: Vec<String> = self
            .metrics
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}",
                    entry.scope.as_str(),
                    entry.name,
                    entry.unit.as_str()
                )
            })
            .collect();
        entries.sort();
        let mut canonical = format!("v{}\n", self.schema_version);
        canonical.push_str(&entries.join("\n"));
        canonical
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cohort {
    id: i64,
}

impl Cohort {
    pub fn from_definition(definition: &SampleDefinition) -> Self {
        let digest = Sha256::digest(definition.canonical().as_bytes());
        let id = i64::from_be_bytes(
            digest[..8]
                .try_into()
                .expect("sha256 digest has eight leading bytes"),
        );
        Self { id: id & i64::MAX }
    }

    pub const fn id(self) -> i64 {
        self.id
    }
}

#[derive(Debug, Clone)]
pub struct CohortRegistry {
    cohorts: HashMap<u32, Cohort>,
}

impl Default for CohortRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CohortRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            cohorts: HashMap::new(),
        };
        registry.register(SampleDefinition::v1());
        registry
    }

    pub fn register(&mut self, definition: SampleDefinition) -> Cohort {
        let version = definition.schema_version;
        let cohort = Cohort::from_definition(&definition);
        self.cohorts.insert(version, cohort);
        cohort
    }

    pub fn for_version(&self, version: u32) -> Option<Cohort> {
        self.cohorts.get(&version).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_cohort_v1_is_stable_and_default() {
        let v1 = SampleDefinition::v1();
        assert_eq!(v1.schema_version, SCHEMA_VERSION);
        let registry = CohortRegistry::new();
        assert_eq!(registry.for_version(SCHEMA_VERSION), Some(Cohort::from_definition(&v1)));
        assert_eq!(registry.for_version(SCHEMA_VERSION), Some(Cohort::from_definition(&v1)));
    }

    #[test]
    fn telemetry_cohort_definition_change_starts_a_new_cohort() {
        let mut changed = SampleDefinition::v1();
        changed.schema_version = SCHEMA_VERSION + 1;
        let v1 = Cohort::from_definition(&SampleDefinition::v1());
        let v2 = Cohort::from_definition(&changed);
        assert_ne!(v1, v2);

        let mut metric_added = SampleDefinition::v1();
        metric_added
            .metrics
            .push(metric(Scope::Guest, "cpu.new_metric", Unit::Cores));
        metric_added.schema_version = SCHEMA_VERSION + 2;
        assert_ne!(v1, Cohort::from_definition(&metric_added));

        let mut registry = CohortRegistry::new();
        assert_ne!(registry.register(changed), v1);
    }

    #[test]
    fn telemetry_cohort_canonical_is_order_independent() {
        let mut definition = SampleDefinition::v1();
        let first = definition.canonical();
        definition.metrics.reverse();
        assert_eq!(first, definition.canonical());
    }
}
