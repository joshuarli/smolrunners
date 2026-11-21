use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Guest,
    Workload,
    Host,
}

impl Scope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Guest => "guest",
            Self::Workload => "workload",
            Self::Host => "host",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "guest" => Some(Self::Guest),
            "workload" => Some(Self::Workload),
            "host" => Some(Self::Host),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[serde(rename = "cores")]
    Cores,
    Percent,
    Bytes,
    Count,
    #[serde(rename = "us")]
    Microseconds,
    #[serde(rename = "ns")]
    Nanoseconds,
    Seconds,
}

impl Unit {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cores => "cores",
            Self::Percent => "percent",
            Self::Bytes => "bytes",
            Self::Count => "count",
            Self::Microseconds => "us",
            Self::Nanoseconds => "ns",
            Self::Seconds => "seconds",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    #[serde(rename = "n")]
    pub name: String,
    #[serde(rename = "v")]
    pub value: f64,
    #[serde(rename = "u")]
    pub unit: Unit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    #[serde(rename = "v")]
    pub version: u32,
    pub scope: Scope,
    #[serde(rename = "seq")]
    pub sequence: u64,
    #[serde(rename = "mono_ns")]
    pub monotonic_ns: u64,
    #[serde(rename = "wall_ms")]
    pub wall_unix_ms: i64,
    #[serde(rename = "boot")]
    pub boot_id: String,
    #[serde(rename = "gen")]
    pub generation: u64,
    pub metrics: Vec<Metric>,
}

impl Sample {
    pub fn metric(&self, name: &str) -> Option<f64> {
        self.metrics
            .iter()
            .find(|metric| metric.name == name)
            .map(|metric| metric.value)
    }

    pub fn metric_unit(&self, name: &str) -> Option<Unit> {
        self.metrics
            .iter()
            .find(|metric| metric.name == name)
            .map(|metric| metric.unit)
    }

    pub fn identity(&self) -> (&str, u64) {
        (self.boot_id.as_str(), self.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Sample {
        Sample {
            version: SCHEMA_VERSION,
            scope: Scope::Guest,
            sequence: 7,
            monotonic_ns: 1_250_000_000,
            wall_unix_ms: 1_733_000_000_007,
            boot_id: "boot-abc".to_owned(),
            generation: 2,
            metrics: vec![
                Metric {
                    name: "cpu.busy_cores".to_owned(),
                    value: 1.5,
                    unit: Unit::Cores,
                },
                Metric {
                    name: "mem.used_bytes".to_owned(),
                    value: 4096.0,
                    unit: Unit::Bytes,
                },
            ],
        }
    }

    #[test]
    fn schema_uses_compact_wire_field_names() {
        let json = serde_json::to_string(&sample()).expect("encode sample");
        assert!(json.contains("\"seq\":7"), "{json}");
        assert!(json.contains("\"mono_ns\":1250000000"), "{json}");
        assert!(json.contains("\"wall_ms\":1733000000007"), "{json}");
        assert!(json.contains("\"boot\":\"boot-abc\""), "{json}");
        assert!(json.contains("\"gen\":2"), "{json}");
        assert!(json.contains("\"n\":\"cpu.busy_cores\""), "{json}");
        assert!(json.contains("\"u\":\"cores\""), "{json}");
        let decoded: Sample = serde_json::from_str(&json).expect("decode sample");
        assert_eq!(decoded, sample());
    }

    #[test]
    fn schema_metric_lookup_reports_value_and_unit() {
        let sample = sample();
        assert_eq!(sample.metric("cpu.busy_cores"), Some(1.5));
        assert_eq!(sample.metric_unit("mem.used_bytes"), Some(Unit::Bytes));
        assert_eq!(sample.metric_unit("missing"), None);
    }

    #[test]
    fn schema_scope_round_trips_through_text() {
        for scope in [Scope::Guest, Scope::Workload, Scope::Host] {
            assert_eq!(Scope::parse(scope.as_str()), Some(scope));
        }
        assert_eq!(Scope::parse("vmm"), None);
    }
}
