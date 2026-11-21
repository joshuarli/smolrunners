use std::collections::HashMap;

use super::schema::{Sample, Scope};

#[derive(Debug, Clone, PartialEq)]
pub struct Rate {
    pub scope: Scope,
    pub name: String,
    pub per_second: f64,
}

pub fn delta(previous: &Sample, current: &Sample, name: &str) -> Option<f64> {
    if previous.version != current.version || previous.identity() != current.identity() {
        return None;
    }
    let before = previous.metric(name)?;
    let after = current.metric(name)?;
    if after < before {
        return None;
    }
    Some(after - before)
}

pub fn rate(previous: &Sample, current: &Sample, name: &str) -> Option<f64> {
    if current.monotonic_ns <= previous.monotonic_ns {
        return None;
    }
    let change = delta(previous, current, name)?;
    let seconds = (current.monotonic_ns - previous.monotonic_ns) as f64 / 1_000_000_000.0;
    Some(change / seconds)
}

#[derive(Debug, Clone)]
pub struct Series {
    samples: Vec<Sample>,
    gaps: usize,
    peaks: HashMap<(Scope, String), f64>,
}

impl Default for Series {
    fn default() -> Self {
        Self::new()
    }
}

impl Series {
    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
            gaps: 0,
            peaks: HashMap::new(),
        }
    }

    pub fn ingest(&mut self, sample: Sample) {
        if let Some(last) = self.samples.last() {
            let identity_changed = last.identity() != sample.identity();
            let sequence_gap = sample.sequence != last.sequence.wrapping_add(1);
            let clock_went_backwards = sample.monotonic_ns <= last.monotonic_ns;
            if identity_changed || sequence_gap || clock_went_backwards {
                self.gaps += 1;
            }
        }
        for metric in &sample.metrics {
            let peak = self
                .peaks
                .entry((sample.scope, metric.name.clone()))
                .or_insert(f64::NEG_INFINITY);
            if metric.value > *peak {
                *peak = metric.value;
            }
        }
        self.samples.push(sample);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn gaps(&self) -> usize {
        self.gaps
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    pub fn first_sequence(&self) -> Option<u64> {
        self.samples.first().map(|sample| sample.sequence)
    }

    pub fn last_sequence(&self) -> Option<u64> {
        self.samples.last().map(|sample| sample.sequence)
    }

    pub fn peak(&self, scope: Scope, name: &str) -> Option<f64> {
        self.peaks
            .get(&(scope, name.to_owned()))
            .copied()
            .filter(|value| value.is_finite())
    }

    pub fn rate_of_last(&self, scope: Scope, name: &str) -> Option<f64> {
        let matching: Vec<&Sample> = self
            .samples
            .iter()
            .filter(|sample| sample.scope == scope)
            .collect();
        let previous = matching.get(matching.len().checked_sub(2)?)?;
        let current = matching.last()?;
        rate(previous, current, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::schema::{Metric, Unit, SCHEMA_VERSION};

    fn sample(scope: Scope, sequence: u64, monotonic_ns: u64, boot: &str, generation: u64) -> Sample {
        Sample {
            version: SCHEMA_VERSION,
            scope,
            sequence,
            monotonic_ns,
            wall_unix_ms: 1_700_000_000_000 + sequence as i64 * 1000,
            boot_id: boot.to_owned(),
            generation,
            metrics: vec![Metric {
                name: "counter".to_owned(),
                value: sequence as f64 * 10.0,
                unit: Unit::Count,
            }],
        }
    }

    #[test]
    fn aggregate_rate_uses_monotonic_denominator_not_wall_clock() {
        let mut previous = sample(Scope::Guest, 1, 1_000_000_000, "boot", 1);
        let mut current = sample(Scope::Guest, 2, 3_000_000_000, "boot", 1);
        previous.wall_unix_ms = 9_000_000_000_000;
        current.wall_unix_ms = 1_000_000_000_000;
        assert_eq!(rate(&previous, &current, "counter"), Some(5.0));

        let mut stalled = current.clone();
        stalled.monotonic_ns = current.monotonic_ns;
        assert_eq!(rate(&current, &stalled, "counter"), None);
    }

    #[test]
    fn aggregate_rejects_counter_reset_and_identity_change() {
        let previous = sample(Scope::Workload, 1, 1_000_000_000, "boot", 1);
        let mut reset = sample(Scope::Workload, 2, 2_000_000_000, "boot", 1);
        reset.metrics[0].value = 0.0;
        assert_eq!(rate(&previous, &reset, "counter"), None);
        assert_eq!(delta(&previous, &reset, "counter"), None);

        let new_boot = sample(Scope::Workload, 2, 2_000_000_000, "boot-2", 1);
        assert_eq!(rate(&previous, &new_boot, "counter"), None);
        let new_generation = sample(Scope::Workload, 2, 2_000_000_000, "boot", 2);
        assert_eq!(rate(&previous, &new_generation, "counter"), None);
    }

    #[test]
    fn aggregate_series_records_gaps_peaks_and_sequences() {
        let mut series = Series::new();
        series.ingest(sample(Scope::Guest, 1, 1_000_000_000, "boot", 1));
        series.ingest(sample(Scope::Guest, 2, 2_000_000_000, "boot", 1));
        let mut workload = sample(Scope::Workload, 3, 3_000_000_000, "boot", 1);
        workload.metrics[0].value = 99.0;
        series.ingest(workload);
        series.ingest(sample(Scope::Guest, 9, 4_000_000_000, "boot", 1));

        assert_eq!(series.len(), 4);
        assert_eq!(series.first_sequence(), Some(1));
        assert_eq!(series.last_sequence(), Some(9));
        assert_eq!(series.gaps(), 1);
        assert_eq!(series.peak(Scope::Guest, "counter"), Some(90.0));
        assert_eq!(series.peak(Scope::Workload, "counter"), Some(99.0));
        assert_eq!(series.peak(Scope::Host, "counter"), None);
    }

    #[test]
    fn aggregate_series_computes_rate_for_the_last_sample_of_a_scope() {
        let mut series = Series::new();
        series.ingest(sample(Scope::Guest, 1, 1_000_000_000, "boot", 1));
        series.ingest(sample(Scope::Host, 2, 1_500_000_000, "boot", 1));
        series.ingest(sample(Scope::Guest, 3, 3_000_000_000, "boot", 1));
        assert_eq!(series.rate_of_last(Scope::Guest, "counter"), Some(10.0));
        assert_eq!(series.rate_of_last(Scope::Host, "counter"), None);
    }
}
