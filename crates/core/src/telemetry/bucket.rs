use std::collections::HashMap;

use super::cohort::Cohort;
use super::schema::{Sample, Scope, Unit};

#[derive(Debug, Clone)]
pub struct Observation {
    pub sample: Sample,
    pub gap_before: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bucket {
    pub scope: Scope,
    pub metric: String,
    pub unit: Unit,
    pub cohort: Cohort,
    pub execution_attempt_id: Option<i64>,
    pub start_ns: u64,
    pub end_ns: u64,
    pub count: usize,
    pub coverage_ns: u64,
    pub min: f64,
    pub max: f64,
    pub sum: f64,
    pub first: f64,
    pub last: f64,
    pub gap_count: usize,
    pub has_gap: bool,
}

pub fn bucketize(
    observations: &[Observation],
    width_ns: u64,
    cohort: Cohort,
    execution_attempt_id: Option<i64>,
) -> Vec<Bucket> {
    let mut groups: HashMap<(Scope, String, u64), Vec<&Observation>> = HashMap::new();
    for observation in observations {
        let slot = if width_ns == 0 {
            0
        } else {
            observation.sample.monotonic_ns / width_ns
        };
        for metric in &observation.sample.metrics {
            groups
                .entry((observation.sample.scope, metric.name.clone(), slot))
                .or_default()
                .push(observation);
        }
    }

    let mut buckets = Vec::with_capacity(groups.len());
    for ((scope, metric, _slot), mut entries) in groups {
        entries.sort_by_key(|entry| entry.sample.monotonic_ns);
        let unit = entries
            .first()
            .and_then(|entry| entry.sample.metric_unit(&metric))
            .expect("bucket metric has a unit");

        let mut values: Vec<(u64, f64)> = Vec::with_capacity(entries.len());
        let mut coverage_ns = 0u64;
        let mut gap_count = 0usize;
        let mut previous: Option<&Observation> = None;
        for entry in &entries {
            let value = entry
                .sample
                .metric(&metric)
                .expect("bucket metric exists in sample");
            values.push((entry.sample.monotonic_ns, value));
            if entry.gap_before {
                gap_count += 1;
            }
            if let Some(before) = previous {
                let joined = !entry.gap_before
                    && before.sample.identity() == entry.sample.identity()
                    && entry.sample.sequence == before.sample.sequence.wrapping_add(1)
                    && entry.sample.monotonic_ns > before.sample.monotonic_ns;
                if joined {
                    coverage_ns += entry.sample.monotonic_ns - before.sample.monotonic_ns;
                }
            }
            previous = Some(entry);
        }

        let first = values.first().expect("bucket has a value").1;
        let last = values.last().expect("bucket has a value").1;
        let mut min = first;
        let mut max = first;
        let mut sum = 0.0;
        for (_at, value) in &values {
            min = min.min(*value);
            max = max.max(*value);
            sum += *value;
        }

        buckets.push(Bucket {
            scope,
            metric,
            unit,
            cohort,
            execution_attempt_id,
            start_ns: values.first().expect("bucket has a start").0,
            end_ns: values.last().expect("bucket has an end").0,
            count: values.len(),
            coverage_ns,
            min,
            max,
            sum,
            first,
            last,
            gap_count,
            has_gap: gap_count > 0,
        });
    }
    buckets.sort_by(|left, right| {
        (left.scope.as_str(), &left.metric, left.start_ns).cmp(&(
            right.scope.as_str(),
            &right.metric,
            right.start_ns,
        ))
    });
    buckets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::schema::{Metric, Unit, SCHEMA_VERSION};

    fn observation(sequence: u64, monotonic_ns: u64, value: f64, gap_before: bool) -> Observation {
        Observation {
            gap_before,
            sample: Sample {
                version: SCHEMA_VERSION,
                scope: Scope::Guest,
                sequence,
                monotonic_ns,
                wall_unix_ms: 1_700_000_000_000 + sequence as i64 * 100,
                boot_id: "boot".to_owned(),
                generation: 1,
                metrics: vec![Metric {
                    name: "counter".to_owned(),
                    value,
                    unit: Unit::Cores,
                }],
            },
        }
    }

    #[test]
    fn telemetry_bucket_preserves_peak_and_reports_all_statistics() {
        let observations = vec![
            observation(1, 1_000_000_000, 1.0, false),
            observation(2, 1_100_000_000, 5.0, false),
            observation(3, 1_200_000_000, 3.0, false),
        ];
        let buckets = bucketize(&observations, 2_000_000_000, Cohort::from_definition(
            &crate::telemetry::cohort::SampleDefinition::v1(),
        ), Some(7));
        assert_eq!(buckets.len(), 1);
        let bucket = &buckets[0];
        assert_eq!(bucket.count, 3);
        assert_eq!(bucket.coverage_ns, 200_000_000);
        assert_eq!(bucket.min, 1.0);
        assert_eq!(bucket.max, 5.0);
        assert_eq!(bucket.sum, 9.0);
        assert_eq!(bucket.first, 1.0);
        assert_eq!(bucket.last, 3.0);
        assert_eq!(bucket.start_ns, 1_000_000_000);
        assert_eq!(bucket.end_ns, 1_200_000_000);
        assert_eq!(bucket.execution_attempt_id, Some(7));
        assert!(!bucket.has_gap);
    }

    #[test]
    fn telemetry_bucket_flags_a_gap_without_zero_filling() {
        let observations = vec![
            observation(1, 1_000_000_000, 1.0, false),
            observation(9, 3_000_000_000, 7.0, true),
        ];
        let buckets = bucketize(&observations, 4_000_000_000, Cohort::from_definition(
            &crate::telemetry::cohort::SampleDefinition::v1(),
        ), None);
        assert_eq!(buckets.len(), 1);
        let bucket = &buckets[0];
        assert!(bucket.has_gap);
        assert_eq!(bucket.gap_count, 1);
        assert_eq!(bucket.count, 2);
        assert_eq!(bucket.coverage_ns, 0);
        assert_eq!(bucket.max, 7.0);
        assert!(bucket.min > 0.0);
    }
}
