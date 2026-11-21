use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as ShaDigest, Sha256};

use crate::cache::error::Result;
use crate::cache::scope::TrustTier;
use crate::store::{Store, StoreError};

pub const OUTCOME_EVENT_KIND: &str = "cache-outcome";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fallback {
    pub reason: String,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheOutcome {
    pub schema_version: u32,
    pub job: String,
    pub repository: String,
    pub trust_tier: String,
    pub generation: Option<String>,
    pub private_view: Option<String>,
    pub hit: bool,
    pub fallback: Option<Fallback>,
    pub published_generation: Option<String>,
    pub recorded_at: String,
}

impl CacheOutcome {
    pub fn fallback(job: &str, repository: &str, tier: TrustTier, reason: impl Into<String>, now: &str) -> Self {
        Self {
            schema_version: 1,
            job: job.to_owned(),
            repository: repository.to_owned(),
            trust_tier: tier.as_str().to_owned(),
            generation: None,
            private_view: None,
            hit: false,
            fallback: Some(Fallback {
                reason: reason.into(),
                visible: true,
            }),
            published_generation: None,
            recorded_at: now.to_owned(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct OutcomeRow {
    pub event_id: i64,
    pub job: String,
    pub repository: String,
    pub payload: Value,
    pub occurred_at: String,
}

fn stable_id(text: &str) -> i64 {
    let mut hasher = Sha256::new();
    hasher.update(b"smolrunners-cache:");
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut value = 0i64;
    for byte in &digest[..8] {
        value = (value << 8) | *byte as i64;
    }
    value & 0x7fff_ffff_ffff_ffff
}

fn ensure_job(store: &Store, repository: &str, job: &str, now: &str) -> Result<i64> {
    let connection = store.connection();
    let installation_id = stable_id("installation");
    connection.execute(
        "INSERT OR IGNORE INTO github_installation
            (id, installation_id, app_id, account_login, scope, runner_group_id,
             runner_group_name, created_at, updated_at)
         VALUES (?1, ?2, 1, 'cache', 'repository', 1, 'cache', ?3, ?3)",
        params![installation_id, installation_id, now],
    ).map_err(|error| StoreError::Sqlite(error))?;
    let workflow_run_id = stable_id(&format!("workflow-run:{repository}"));
    connection.execute(
        "INSERT OR IGNORE INTO workflow_run
            (id, installation_id, repository, workflow_name, workflow_run_id,
             head_branch, created_at)
         VALUES (?1, ?2, ?3, 'cache', ?4, 'main', ?5)",
        params![workflow_run_id, installation_id, repository, workflow_run_id, now],
    ).map_err(|error| StoreError::Sqlite(error))?;
    let workflow_run_row: i64 = connection.query_row(
        "SELECT id FROM workflow_run WHERE installation_id = ?1 AND workflow_run_id = ?2",
        params![installation_id, workflow_run_id],
        |row| row.get(0),
    ).map_err(|error| StoreError::Sqlite(error))?;
    connection.execute(
        "INSERT OR IGNORE INTO workflow_run_attempt
            (workflow_run_id, attempt, created_at)
         VALUES (?1, 1, ?2)",
        params![workflow_run_row, now],
    ).map_err(|error| StoreError::Sqlite(error))?;
    let attempt_row: i64 = connection.query_row(
        "SELECT id FROM workflow_run_attempt WHERE workflow_run_id = ?1 AND attempt = 1",
        params![workflow_run_row],
        |row| row.get(0),
    ).map_err(|error| StoreError::Sqlite(error))?;
    let job_id = stable_id(&format!("job:{repository}:{job}"));
    connection.execute(
        "INSERT OR IGNORE INTO job
            (id, workflow_run_attempt_id, github_job_key, name, created_at)
         VALUES (?1, ?2, ?3, ?3, ?4)",
        params![job_id, attempt_row, job, now],
    ).map_err(|error| StoreError::Sqlite(error))?;
    let job_row: i64 = connection.query_row(
        "SELECT id FROM job WHERE workflow_run_attempt_id = ?1 AND github_job_key = ?2",
        params![attempt_row, job],
        |row| row.get(0),
    ).map_err(|error| StoreError::Sqlite(error))?;
    connection.execute(
        "INSERT OR IGNORE INTO execution_attempt
            (job_id, attempt, state, started_at)
         VALUES (?1, 1, 'reserved', ?2)",
        params![job_row, now],
    ).map_err(|error| StoreError::Sqlite(error))?;
    let execution_id: i64 = connection.query_row(
        "SELECT id FROM execution_attempt WHERE job_id = ?1 AND attempt = 1",
        params![job_row],
        |row| row.get(0),
    ).map_err(|error| StoreError::Sqlite(error))?;
    Ok(execution_id)
}

pub fn record_outcome(store: &Store, outcome: &CacheOutcome) -> Result<i64> {
    let execution_id = ensure_job(store, &outcome.repository, &outcome.job, &outcome.recorded_at)?;
    let payload = serde_json::to_string(outcome)?;
    store.connection().execute(
        "INSERT INTO event (execution_attempt_id, kind, payload_json, occurred_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![execution_id, OUTCOME_EVENT_KIND, payload, outcome.recorded_at],
    ).map_err(|error| StoreError::Sqlite(error))?;
    Ok(execution_id)
}

pub fn read_outcomes(store: &Store) -> Result<Vec<OutcomeRow>> {
    let mut statement = store.connection().prepare(
        "SELECT e.id, j.github_job_key, wr.repository, e.payload_json, e.occurred_at
         FROM event e
         JOIN execution_attempt ea ON ea.id = e.execution_attempt_id
         JOIN job j ON j.id = ea.job_id
         JOIN workflow_run_attempt wra ON wra.id = j.workflow_run_attempt_id
         JOIN workflow_run wr ON wr.id = wra.workflow_run_id
         WHERE e.kind = ?1
         ORDER BY e.id",
    ).map_err(|error| StoreError::Sqlite(error))?;
    let rows = statement.query_map(params![OUTCOME_EVENT_KIND], |row| {
        let payload: String = row.get(3)?;
        Ok(OutcomeRow {
            event_id: row.get(0)?,
            job: row.get(1)?,
            repository: row.get(2)?,
            payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            occurred_at: row.get(4)?,
        })
    }).map_err(|error| StoreError::Sqlite(error))?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|error| StoreError::Sqlite(error))?);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Store, test_support};

    fn outcome(job: &str, hit: bool) -> CacheOutcome {
        CacheOutcome {
            schema_version: 1,
            job: job.to_owned(),
            repository: "acme/widgets".to_owned(),
            trust_tier: "maintainer".to_owned(),
            generation: Some("gen-abc".to_owned()),
            private_view: Some(format!("views/{job}")),
            hit,
            fallback: None,
            published_generation: None,
            recorded_at: "2026-11-21T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn cache_outcome_fields_are_recorded_in_job_history() {
        let path = test_support::temp_db_path("outcome_history");
        let store = Store::open(&path).expect("open store");
        record_outcome(&store, &outcome("job-a", true)).expect("record a");
        record_outcome(&store, &outcome("job-b", false)).expect("record b");
        let rows = read_outcomes(&store).expect("read outcomes");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].job, "job-a");
        assert_eq!(rows[0].repository, "acme/widgets");
        assert_eq!(rows[0].payload["hit"], Value::Bool(true));
        assert_eq!(rows[0].payload["generation"], Value::String("gen-abc".to_owned()));
        assert_eq!(rows[0].payload["private_view"], Value::String("views/job-a".to_owned()));
        assert_eq!(rows[1].job, "job-b");
        test_support::remove_db(&path);
    }

    #[test]
    fn cache_fallback_outcome_carries_a_visible_reason() {
        let path = test_support::temp_db_path("outcome_fallback");
        let store = Store::open(&path).expect("open store");
        let fallback = CacheOutcome::fallback(
            "job-c",
            "acme/widgets",
            TrustTier::Contributor,
            "read would broaden trust tier from contributor to maintainer",
            "2026-11-21T00:00:00Z",
        );
        record_outcome(&store, &fallback).expect("record fallback");
        let rows = read_outcomes(&store).expect("read outcomes");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].payload["hit"], Value::Bool(false));
        assert_eq!(rows[0].payload["fallback"]["visible"], Value::Bool(true));
        assert!(rows[0].payload["fallback"]["reason"].as_str().unwrap().contains("broaden"));
        test_support::remove_db(&path);
    }
}
