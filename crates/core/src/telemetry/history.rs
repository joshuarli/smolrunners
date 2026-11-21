use std::collections::HashMap;
use std::path::Path;

use rusqlite::params;

use crate::store::{Result as StoreResult, SampleRow, StoreError, TelemetryWriter, open_connection};

use super::aggregate::{Rate, rate};
use super::bucket::{Bucket, Observation, bucketize};
use super::cohort::{Cohort, CohortRegistry, SampleDefinition};
use super::schema::{Sample, Scope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapReason {
    NewCohort,
    NewIncarnation,
    Sequence,
    ClockReset,
}

#[derive(Debug, Clone, Default)]
pub struct IngestContext {
    pub host_id: Option<i64>,
    pub execution_attempt_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IngestOutcome {
    pub cohort: Cohort,
    pub gap: bool,
    pub gap_reason: Option<GapReason>,
    pub rates: Vec<Rate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StreamKey {
    execution_attempt_id: Option<i64>,
    cohort: Cohort,
    scope: Scope,
}

struct StreamState {
    last_sequence: u64,
    last_monotonic_ns: u64,
    identity: (String, u64),
    cohort: Cohort,
    observations: Vec<Observation>,
}

pub struct History {
    writer: TelemetryWriter,
    reader: rusqlite::Connection,
    cohorts: CohortRegistry,
    streams: HashMap<StreamKey, StreamState>,
    buckets: Vec<Bucket>,
    gaps: usize,
}

impl History {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::with_capacity(path, 512)
    }

    pub fn with_capacity(path: impl AsRef<Path>, capacity: usize) -> StoreResult<Self> {
        let writer = TelemetryWriter::with_capacity(path.as_ref(), capacity)?;
        let reader = open_connection(path.as_ref())?;
        Ok(Self {
            writer,
            reader,
            cohorts: CohortRegistry::new(),
            streams: HashMap::new(),
            buckets: Vec::new(),
            gaps: 0,
        })
    }

    pub fn register_definition(&mut self, definition: SampleDefinition) {
        self.cohorts.register(definition);
    }

    pub fn cohort_for(&self, version: u32) -> StoreResult<Cohort> {
        self.cohorts.for_version(version).ok_or_else(|| {
            StoreError::Migration(format!("unknown telemetry sample schema version {version}"))
        })
    }

    pub fn ingest(&mut self, sample: Sample, context: IngestContext) -> StoreResult<IngestOutcome> {
        let cohort = self.cohort_for(sample.version)?;
        let captured_at = rfc3339_ms(sample.wall_unix_ms);
        for metric in &sample.metrics {
            self.writer.push(SampleRow {
                scope: sample.scope.as_str().to_owned(),
                metric: metric.name.clone(),
                value: metric.value,
                unit: metric.unit.as_str().to_owned(),
                sequence: sample.sequence as i64,
                captured_at: captured_at.clone(),
                host_id: context.host_id,
                execution_attempt_id: context.execution_attempt_id,
                cohort: cohort.id(),
            })?;
        }

        let key = StreamKey {
            execution_attempt_id: context.execution_attempt_id,
            cohort,
            scope: sample.scope,
        };
        let gap_reason = match self.streams.get(&key) {
            None => None,
            Some(state) => {
                if state.cohort != cohort {
                    Some(GapReason::NewCohort)
                } else if state.identity != (sample.boot_id.clone(), sample.generation) {
                    Some(GapReason::NewIncarnation)
                } else if sample.sequence != state.last_sequence.wrapping_add(1) {
                    Some(GapReason::Sequence)
                } else if sample.monotonic_ns <= state.last_monotonic_ns {
                    Some(GapReason::ClockReset)
                } else {
                    None
                }
            }
        };
        let gap = gap_reason.is_some();
        if gap {
            self.gaps += 1;
        }

        let rates: Vec<Rate> = if gap {
            Vec::new()
        } else {
            match self
                .streams
                .get(&key)
                .and_then(|state| state.observations.last())
            {
                None => Vec::new(),
                Some(previous) => sample
                    .metrics
                    .iter()
                    .filter_map(|metric| {
                        rate(&previous.sample, &sample, &metric.name).map(|per_second| Rate {
                            scope: sample.scope,
                            name: metric.name.clone(),
                            per_second,
                        })
                    })
                    .collect(),
            }
        };

        let identity = (sample.boot_id.clone(), sample.generation);
        let state = self.streams.entry(key).or_insert_with(|| StreamState {
            last_sequence: sample.sequence,
            last_monotonic_ns: sample.monotonic_ns,
            identity: identity.clone(),
            cohort,
            observations: Vec::new(),
        });
        state.last_sequence = sample.sequence;
        state.last_monotonic_ns = sample.monotonic_ns;
        state.identity = identity;
        state.cohort = cohort;
        state.observations.push(Observation {
            sample,
            gap_before: gap,
        });

        Ok(IngestOutcome {
            cohort,
            gap,
            gap_reason,
            rates,
        })
    }

    pub fn flush(&mut self) -> StoreResult<()> {
        self.writer.flush()
    }

    pub fn pending(&self) -> usize {
        self.writer.pending()
    }

    pub fn gaps(&self) -> usize {
        self.gaps
    }

    pub fn buckets(&self, width_ns: u64) -> Vec<Bucket> {
        let mut buckets = Vec::new();
        for (key, state) in &self.streams {
            buckets.extend(bucketize(
                &state.observations,
                width_ns,
                key.cohort,
                key.execution_attempt_id,
            ));
        }
        buckets
    }

    pub fn compact(&mut self, width_ns: u64) -> Vec<Bucket> {
        let produced = self.buckets(width_ns);
        self.buckets.extend(produced.iter().cloned());
        for state in self.streams.values_mut() {
            state.observations.clear();
        }
        produced
    }

    pub fn stored_buckets(&self) -> &[Bucket] {
        &self.buckets
    }

    pub fn retain_before_ms(&mut self, cutoff_ms: i64) -> StoreResult<usize> {
        self.writer.flush()?;
        let cutoff = rfc3339_ms(cutoff_ms);
        let deleted = self
            .reader
            .execute("DELETE FROM sample WHERE captured_at < ?1", params![cutoff])
            .map_err(StoreError::Sqlite)?;
        for state in self.streams.values_mut() {
            state
                .observations
                .retain(|observation| observation.sample.wall_unix_ms >= cutoff_ms);
        }
        Ok(deleted)
    }

    pub fn rows_for(&self, execution_attempt_id: i64) -> StoreResult<Vec<SampleRow>> {
        let mut statement = self
            .reader
            .prepare(
                "SELECT scope, metric, value, unit, sequence, captured_at,
                        host_id, execution_attempt_id, cohort
                   FROM sample
                  WHERE execution_attempt_id = ?1
                  ORDER BY id",
            )
            .map_err(StoreError::Sqlite)?;
        let rows = statement
            .query_map(params![execution_attempt_id], |row| {
                Ok(SampleRow {
                    scope: row.get(0)?,
                    metric: row.get(1)?,
                    value: row.get(2)?,
                    unit: row.get(3)?,
                    sequence: row.get(4)?,
                    captured_at: row.get(5)?,
                    host_id: row.get(6)?,
                    execution_attempt_id: row.get(7)?,
                    cohort: row.get(8)?,
                })
            })
            .map_err(StoreError::Sqlite)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::Sqlite)?;
        Ok(rows)
    }
}

fn rfc3339_ms(wall_ms: i64) -> String {
    let days = wall_ms.div_euclid(86_400_000);
    let ms_of_day = wall_ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hours = ms_of_day / 3_600_000;
    let minutes = (ms_of_day % 3_600_000) / 60_000;
    let seconds = (ms_of_day % 60_000) / 1_000;
    let millis = ms_of_day % 1_000;
    format!(
        "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z"
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524
        - day_of_era / 146_096)
        / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::{remove_db, temp_db_path};
    use crate::telemetry::cohort::SampleDefinition;
    use crate::telemetry::schema::{Metric, Unit, SCHEMA_VERSION};

    fn seed_execution(path: &std::path::Path, execution_id: i64, host_id: i64) {
        let connection = open_connection(path).expect("open seed connection");
        crate::store::migrate(&connection).expect("migrate seed connection");
        connection
            .execute_batch(&format!(
                "INSERT INTO github_installation
                    (id, installation_id, app_id, account_login, scope,
                     runner_group_id, runner_group_name, created_at, updated_at)
                 VALUES (1, 1, 1, 'acct', 'all', 1, 'rg', 'now', 'now');
                 INSERT INTO workflow_run
                    (id, installation_id, repository, workflow_name,
                     workflow_run_id, head_branch, created_at)
                 VALUES (1, 1, 'o/r', 'wf', 1, 'main', 'now');
                 INSERT INTO workflow_run_attempt
                    (id, workflow_run_id, attempt, created_at)
                 VALUES (1, 1, 1, 'now');
                 INSERT INTO job
                    (id, workflow_run_attempt_id, github_job_key, name, created_at)
                 VALUES (1, 1, 'key', 'job', 'now');
                 INSERT INTO execution_attempt
                    (id, job_id, attempt, state, started_at)
                 VALUES ({execution_id}, 1, 1, 'busy', 'now');
                 INSERT INTO host
                    (id, host_key, incarnation, hostname, arch, total_vcpu,
                     total_memory_gib, total_disk_bytes, capabilities_json,
                     admission_state, created_at)
                 VALUES ({host_id}, 'host-{host_id}', 1, 'h', 'arm64', 4, 8, 1000,
                         '{{}}', 'eligible', 'now');"
            ))
            .expect("seed execution and host");
    }

    fn sample(
        scope: Scope,
        sequence: u64,
        monotonic_ns: u64,
        wall_unix_ms: i64,
        boot: &str,
        generation: u64,
        value: f64,
    ) -> Sample {
        Sample {
            version: SCHEMA_VERSION,
            scope,
            sequence,
            monotonic_ns,
            wall_unix_ms,
            boot_id: boot.to_owned(),
            generation,
            metrics: vec![Metric {
                name: "counter".to_owned(),
                value,
                unit: Unit::Count,
            }],
        }
    }

    #[test]
    fn telemetry_history_batches_into_the_store_with_capture_time_labels() {
        let path = temp_db_path("history_batch");
        seed_execution(&path, 9, 1);
        let mut history = History::with_capacity(&path, 2).expect("open history");
        let context = IngestContext {
            host_id: Some(1),
            execution_attempt_id: Some(9),
        };
        let captured = 1_733_000_000_007i64;
        history
            .ingest(sample(Scope::Guest, 1, 1_000_000_000, captured, "boot", 1, 3.0), context.clone())
            .expect("ingest first");
        assert_eq!(history.pending(), 1);
        history
            .ingest(
                sample(Scope::Guest, 2, 1_100_000_000, captured + 100, "boot", 1, 5.0),
                context.clone(),
            )
            .expect("ingest second");
        assert_eq!(history.pending(), 0);

        let rows = history.rows_for(9).expect("read rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].captured_at, "2024-11-30T20:53:20.007Z");
        assert_eq!(rows[0].sequence, 1);
        assert_eq!(rows[0].host_id, Some(1));
        assert_eq!(rows[0].cohort, Cohort::from_definition(&SampleDefinition::v1()).id());
        remove_db(&path);
    }

    #[test]
    fn telemetry_history_computes_rates_from_capture_time_not_receipt_time() {
        let path = temp_db_path("history_rate");
        let mut history = History::open(&path).expect("open history");
        let context = IngestContext::default();
        history
            .ingest(sample(Scope::Guest, 1, 1_000_000_000, 9_000_000_000_000, "boot", 1, 3.0), context.clone())
            .expect("first");
        let outcome = history
            .ingest(sample(Scope::Guest, 2, 3_000_000_000, 1_000_000_000_000, "boot", 1, 13.0), context)
            .expect("second");
        let counter = outcome.rates.iter().find(|value| value.name == "counter");
        assert_eq!(counter.map(|value| value.per_second), Some(5.0));
        remove_db(&path);
    }

    #[test]
    fn telemetry_history_flags_gaps_and_never_computes_across_them() {
        let path = temp_db_path("history_gaps");
        let mut history = History::open(&path).expect("open history");
        let context = IngestContext::default();
        history
            .ingest(sample(Scope::Guest, 1, 1_000_000_000, 1_000, "boot", 1, 3.0), context.clone())
            .expect("first");
        let outage = history
            .ingest(sample(Scope::Guest, 9, 9_000_000_000, 9_000, "boot", 1, 4.0), context.clone())
            .expect("after outage");
        assert!(outage.gap);
        assert_eq!(outage.gap_reason, Some(GapReason::Sequence));
        assert!(outage.rates.is_empty());

        let restart = history
            .ingest(sample(Scope::Guest, 10, 10_000_000_000, 10_000, "boot", 2, 5.0), context)
            .expect("after restart");
        assert!(restart.gap);
        assert_eq!(restart.gap_reason, Some(GapReason::NewIncarnation));
        assert!(restart.rates.is_empty());
        assert_eq!(history.gaps(), 2);
        remove_db(&path);
    }

    #[test]
    fn telemetry_history_compaction_keeps_peaks_and_retention_drops_raw_rows() {
        let path = temp_db_path("history_compaction");
        seed_execution(&path, 4, 1);
        let mut history = History::open(&path).expect("open history");
        let context = IngestContext {
            execution_attempt_id: Some(4),
            host_id: None,
        };
        for index in 1..=4 {
            history
                .ingest(
                    sample(
                        Scope::Guest,
                        index,
                        index * 1_000_000_000,
                        1_733_000_000_000 + index as i64 * 1_000,
                        "boot",
                        1,
                        index as f64 * 2.0,
                    ),
                    context.clone(),
                )
                .expect("ingest");
        }
        let buckets = history.compact(5_000_000_000);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].max, 8.0);
        assert_eq!(buckets[0].count, 4);
        assert!(history.buckets(5_000_000_000).is_empty());
        assert_eq!(history.stored_buckets()[0].max, 8.0);

        let deleted = history
            .retain_before_ms(1_733_000_000_000 + 3_000)
            .expect("retain");
        assert_eq!(deleted, 2);
        assert_eq!(history.rows_for(4).expect("rows").len(), 2);
        remove_db(&path);
    }

    #[test]
    fn telemetry_history_rejects_an_unknown_schema_version() {
        let path = temp_db_path("history_version");
        let mut history = History::open(&path).expect("open history");
        let mut unknown = sample(Scope::Guest, 1, 1, 1, "boot", 1, 1.0);
        unknown.version = SCHEMA_VERSION + 99;
        assert!(history.ingest(unknown, IngestContext::default()).is_err());
        remove_db(&path);
    }

    #[test]
    fn telemetry_history_rfc3339_formats_capture_time() {
        assert_eq!(rfc3339_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_ms(1_733_000_000_007), "2024-11-30T20:53:20.007Z");
    }
}
