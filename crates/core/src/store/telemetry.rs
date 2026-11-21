use std::path::Path;

use rusqlite::{Connection, params};

use crate::store::connection::open_connection;
use crate::store::error::{Result, map_error};
use crate::store::migrations::migrate;

#[derive(Debug, Clone, PartialEq)]
pub struct SampleRow {
    pub scope: String,
    pub metric: String,
    pub value: f64,
    pub unit: String,
    pub sequence: i64,
    pub captured_at: String,
    pub host_id: Option<i64>,
    pub execution_attempt_id: Option<i64>,
    pub cohort: i64,
}

pub struct TelemetryWriter {
    connection: Connection,
    batch: Vec<SampleRow>,
    capacity: usize,
}

impl TelemetryWriter {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::with_capacity(path, 512)
    }

    pub fn with_capacity(path: impl AsRef<Path>, capacity: usize) -> Result<Self> {
        let connection = open_connection(path)?;
        migrate(&connection)?;
        Ok(Self {
            connection,
            batch: Vec::new(),
            capacity: capacity.max(1),
        })
    }

    pub fn push(&mut self, sample: SampleRow) -> Result<()> {
        self.batch.push(sample);
        if self.batch.len() >= self.capacity {
            self.flush()?;
        }
        Ok(())
    }

    pub fn pending(&self) -> usize {
        self.batch.len()
    }

    pub fn flush(&mut self) -> Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|error| map_error("begin telemetry batch", error))?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO sample
                        (host_id, execution_attempt_id, scope, metric, value, unit,
                         sequence, cohort, captured_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .map_err(|error| map_error("prepare telemetry insert", error))?;
            for sample in self.batch.iter() {
                statement
                    .execute(params![
                        sample.host_id,
                        sample.execution_attempt_id,
                        sample.scope,
                        sample.metric,
                        sample.value,
                        sample.unit,
                        sample.sequence,
                        sample.cohort,
                        sample.captured_at,
                    ])
                    .map_err(|error| map_error("insert telemetry sample", error))?;
            }
        }
        transaction
            .commit()
            .map_err(|error| map_error("commit telemetry batch", error))?;
        self.batch.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection::{Store, open_connection};
    use crate::store::test_support::{remove_db, temp_db_path};
    use crate::store::txn::ReadTxn;

    fn sample(metric: &str, sequence: i64) -> SampleRow {
        SampleRow {
            scope: "guest".to_owned(),
            metric: metric.to_owned(),
            value: sequence as f64,
            unit: "cores".to_owned(),
            sequence,
            captured_at: format!("2026-11-21T00:00:{sequence:02}Z"),
            host_id: None,
            execution_attempt_id: None,
            cohort: 1,
        }
    }

    fn count_samples(connection: &Connection) -> i64 {
        connection
            .query_row("SELECT COUNT(*) FROM sample", [], |row| row.get(0))
            .expect("count samples")
    }

    #[test]
    fn store_telemetry_batches_commit_independently_of_lifecycle_reads() {
        let path = temp_db_path("telemetry_independent");
        let mut store = Store::open(&path).expect("open lifecycle store");
        let mut writer = TelemetryWriter::with_capacity(&path, 3).expect("open telemetry writer");
        let observer = open_connection(&path).expect("open observer");

        writer.push(sample("cpu", 1)).expect("buffer first sample");
        writer.push(sample("cpu", 2)).expect("buffer second sample");
        assert_eq!(writer.pending(), 2);
        assert_eq!(count_samples(&observer), 0);

        let mut lifecycle_read =
            ReadTxn::begin(store.connection_mut(), 10).expect("begin lifecycle read");
        let _: Vec<i64> = lifecycle_read
            .query("SELECT 1", [])
            .expect("hold a lifecycle read transaction");

        writer.flush().expect("flush telemetry batch");
        assert_eq!(count_samples(&observer), 2);

        lifecycle_read.commit().expect("commit lifecycle read");
        assert_eq!(count_samples(&observer), 2);
        remove_db(&path);
    }

    #[test]
    fn store_telemetry_flushes_when_the_batch_reaches_capacity() {
        let path = temp_db_path("telemetry_capacity");
        let mut writer = TelemetryWriter::with_capacity(&path, 2).expect("open telemetry writer");
        let observer = open_connection(&path).expect("open observer");

        writer.push(sample("cpu", 1)).expect("buffer first");
        assert_eq!(count_samples(&observer), 0);
        writer.push(sample("cpu", 2)).expect("buffer second and flush");
        assert_eq!(count_samples(&observer), 2);
        assert_eq!(writer.pending(), 0);

        writer.push(sample("cpu", 3)).expect("buffer third");
        assert_eq!(writer.pending(), 1);
        assert_eq!(count_samples(&observer), 2);
        writer.flush().expect("flush remaining sample");
        assert_eq!(count_samples(&observer), 3);
        remove_db(&path);
    }
}
