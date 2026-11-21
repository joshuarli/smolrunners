use rusqlite::{Connection, OptionalExtension, params};

use crate::store::{Store, StoreError};

pub const INBOX_RECEIVED: &str = "received";
pub const INBOX_PROCESSING: &str = "processing";
pub const INBOX_DONE: &str = "done";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxEvent {
    pub event_id: String,
    pub kind: String,
    pub payload_json: String,
    pub state: String,
    pub attempts: i64,
    pub decision_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquisitionRow {
    pub event_id: String,
    pub request_ids_json: String,
    pub acquired_ids_json: String,
    pub effect_count: i64,
    pub recorded_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationRow {
    pub event_id: String,
    pub kind: String,
    pub detail_json: String,
    pub observed_at: String,
}

pub fn initialize(connection: &Connection) -> Result<(), StoreError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS lifecycle_execution (
                 execution_key TEXT PRIMARY KEY,
                 job_id TEXT,
                 state TEXT NOT NULL,
                 reservation_id INTEGER,
                 updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS lifecycle_acquisition (
                 event_id TEXT PRIMARY KEY,
                 request_ids_json TEXT NOT NULL,
                 acquired_ids_json TEXT NOT NULL,
                 effect_count INTEGER NOT NULL,
                 recorded_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS lifecycle_observation (
                 event_id TEXT PRIMARY KEY,
                 kind TEXT NOT NULL,
                 detail_json TEXT NOT NULL,
                 observed_at TEXT NOT NULL
             );",
        )
        .map_err(StoreError::Sqlite)
}

pub fn insert_event(
    connection: &Connection,
    event_id: &str,
    kind: &str,
    payload_json: &str,
    now: &str,
) -> Result<bool, StoreError> {
    let changed = connection
        .execute(
            "INSERT OR IGNORE INTO inbox
                 (event_id, kind, payload_json, state, attempts, received_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            params![event_id, kind, payload_json, INBOX_RECEIVED, now],
        )
        .map_err(StoreError::Sqlite)?;
    Ok(changed == 1)
}

pub fn load_event(
    connection: &Connection,
    event_id: &str,
) -> Result<Option<InboxEvent>, StoreError> {
    connection
        .query_row(
            "SELECT event_id, kind, payload_json, state, attempts, decision_json
             FROM inbox WHERE event_id = ?1",
            params![event_id],
            |row| {
                Ok(InboxEvent {
                    event_id: row.get(0)?,
                    kind: row.get(1)?,
                    payload_json: row.get(2)?,
                    state: row.get(3)?,
                    attempts: row.get(4)?,
                    decision_json: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::Sqlite)
}

pub fn mark_processing(connection: &Connection, event_id: &str) -> Result<(), StoreError> {
    connection
        .execute(
            "UPDATE inbox SET state = ?2, attempts = attempts + 1 WHERE event_id = ?1",
            params![event_id, INBOX_PROCESSING],
        )
        .map_err(StoreError::Sqlite)?;
    Ok(())
}

pub fn mark_done(
    connection: &Connection,
    event_id: &str,
    decision_json: &str,
    now: &str,
) -> Result<(), StoreError> {
    connection
        .execute(
            "UPDATE inbox SET state = ?2, decision_json = ?3, processed_at = ?4
             WHERE event_id = ?1",
            params![event_id, INBOX_DONE, decision_json, now],
        )
        .map_err(StoreError::Sqlite)?;
    Ok(())
}

pub fn processing_event_ids(connection: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = connection
        .prepare("SELECT event_id FROM inbox WHERE state = ?1 ORDER BY event_id")
        .map_err(StoreError::Sqlite)?;
    let rows = statement
        .query_map(params![INBOX_PROCESSING], |row| row.get::<_, String>(0))
        .map_err(StoreError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::Sqlite)
}

pub fn upsert_execution(
    connection: &Connection,
    execution_key: &str,
    job_id: Option<&str>,
    state: &str,
    reservation_id: Option<i64>,
    now: &str,
) -> Result<(), StoreError> {
    connection
        .execute(
            "INSERT INTO lifecycle_execution
                 (execution_key, job_id, state, reservation_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(execution_key) DO UPDATE SET
                 job_id = excluded.job_id,
                 state = excluded.state,
                 reservation_id = excluded.reservation_id,
                 updated_at = excluded.updated_at",
            params![execution_key, job_id, state, reservation_id, now],
        )
        .map_err(StoreError::Sqlite)?;
    Ok(())
}

pub fn load_execution_state(
    connection: &Connection,
    execution_key: &str,
) -> Result<Option<String>, StoreError> {
    connection
        .query_row(
            "SELECT state FROM lifecycle_execution WHERE execution_key = ?1",
            params![execution_key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(StoreError::Sqlite)
}

pub fn insert_acquisition(
    connection: &Connection,
    event_id: &str,
    request_ids_json: &str,
    acquired_ids_json: &str,
    effect_count: i64,
    now: &str,
) -> Result<(), StoreError> {
    connection
        .execute(
            "INSERT OR IGNORE INTO lifecycle_acquisition
                 (event_id, request_ids_json, acquired_ids_json, effect_count, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![event_id, request_ids_json, acquired_ids_json, effect_count, now],
        )
        .map_err(StoreError::Sqlite)?;
    Ok(())
}

pub fn load_acquisition(
    connection: &Connection,
    event_id: &str,
) -> Result<Option<AcquisitionRow>, StoreError> {
    connection
        .query_row(
            "SELECT event_id, request_ids_json, acquired_ids_json, effect_count, recorded_at
             FROM lifecycle_acquisition WHERE event_id = ?1",
            params![event_id],
            |row| {
                Ok(AcquisitionRow {
                    event_id: row.get(0)?,
                    request_ids_json: row.get(1)?,
                    acquired_ids_json: row.get(2)?,
                    effect_count: row.get(3)?,
                    recorded_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::Sqlite)
}

pub fn acquisition_count(connection: &Connection) -> Result<i64, StoreError> {
    connection
        .query_row("SELECT COUNT(*) FROM lifecycle_acquisition", [], |row| row.get(0))
        .map_err(StoreError::Sqlite)
}

pub fn insert_observation(
    connection: &Connection,
    event_id: &str,
    kind: &str,
    detail_json: &str,
    now: &str,
) -> Result<(), StoreError> {
    connection
        .execute(
            "INSERT OR IGNORE INTO lifecycle_observation
                 (event_id, kind, detail_json, observed_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![event_id, kind, detail_json, now],
        )
        .map_err(StoreError::Sqlite)?;
    Ok(())
}

pub fn load_observation(
    connection: &Connection,
    event_id: &str,
) -> Result<Option<ObservationRow>, StoreError> {
    connection
        .query_row(
            "SELECT event_id, kind, detail_json, observed_at
             FROM lifecycle_observation WHERE event_id = ?1",
            params![event_id],
            |row| {
                Ok(ObservationRow {
                    event_id: row.get(0)?,
                    kind: row.get(1)?,
                    detail_json: row.get(2)?,
                    observed_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::Sqlite)
}

pub fn open(path: impl AsRef<std::path::Path>) -> Result<Store, StoreError> {
    let store = Store::open(path)?;
    initialize(store.connection())?;
    Ok(store)
}
