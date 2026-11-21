use rusqlite::Connection;

use crate::store::error::{Result, StoreError, map_error};

pub const SCHEMA_VERSION: u32 = 1;

pub const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS github_installation (
    id INTEGER PRIMARY KEY,
    installation_id INTEGER NOT NULL UNIQUE,
    app_id INTEGER NOT NULL,
    account_login TEXT NOT NULL,
    scope TEXT NOT NULL,
    runner_group_id INTEGER NOT NULL,
    runner_group_name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS profile (
    name TEXT PRIMARY KEY,
    current_revision INTEGER NOT NULL,
    promoted_revision INTEGER,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS profile_revision (
    id TEXT PRIMARY KEY,
    profile_name TEXT NOT NULL REFERENCES profile(name),
    revision_number INTEGER NOT NULL,
    distro TEXT NOT NULL,
    release TEXT NOT NULL,
    arch TEXT NOT NULL,
    vcpu INTEGER NOT NULL,
    memory_gib INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (profile_name, revision_number)
);

CREATE TRIGGER IF NOT EXISTS profile_revision_no_update
BEFORE UPDATE ON profile_revision
BEGIN
    SELECT RAISE(ABORT, 'profile revision is immutable');
END;

CREATE TRIGGER IF NOT EXISTS profile_revision_no_delete
BEFORE DELETE ON profile_revision
BEGIN
    SELECT RAISE(ABORT, 'profile revision is immutable');
END;

CREATE TABLE IF NOT EXISTS image (
    id INTEGER PRIMARY KEY,
    digest TEXT NOT NULL UNIQUE,
    distro TEXT NOT NULL,
    release TEXT NOT NULL,
    arch TEXT NOT NULL,
    source TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    built_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS runtime_identity (
    id INTEGER PRIMARY KEY,
    image_id INTEGER NOT NULL REFERENCES image(id),
    runner_version TEXT NOT NULL,
    dotnet_target TEXT NOT NULL,
    node_versions_json TEXT NOT NULL,
    collector_version TEXT NOT NULL,
    recorded_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS image_promotion (
    id INTEGER PRIMARY KEY,
    image_id INTEGER NOT NULL REFERENCES image(id),
    runtime_identity_id INTEGER REFERENCES runtime_identity(id),
    from_stage TEXT,
    to_stage TEXT NOT NULL,
    actor TEXT NOT NULL,
    promoted_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS image_channel (
    channel TEXT PRIMARY KEY,
    image_id INTEGER NOT NULL REFERENCES image(id),
    runtime_identity_id INTEGER NOT NULL REFERENCES runtime_identity(id),
    promoted_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS host (
    id INTEGER PRIMARY KEY,
    host_key TEXT NOT NULL UNIQUE,
    incarnation INTEGER NOT NULL,
    hostname TEXT NOT NULL,
    arch TEXT NOT NULL,
    total_vcpu INTEGER NOT NULL,
    total_memory_gib INTEGER NOT NULL,
    total_disk_bytes INTEGER NOT NULL,
    capabilities_json TEXT NOT NULL,
    admission_state TEXT NOT NULL,
    admission_reason TEXT,
    last_contact_at TEXT,
    drain_revision INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS reservation (
    id INTEGER PRIMARY KEY,
    host_id INTEGER NOT NULL REFERENCES host(id),
    incarnation INTEGER NOT NULL,
    profile_revision_id TEXT NOT NULL,
    vcpu INTEGER NOT NULL,
    memory_gib INTEGER NOT NULL,
    disk_bytes INTEGER NOT NULL,
    state TEXT NOT NULL,
    created_at TEXT NOT NULL,
    released_at TEXT
);

CREATE TABLE IF NOT EXISTS vm_instance (
    id INTEGER PRIMARY KEY,
    vm_key TEXT NOT NULL UNIQUE,
    reservation_id INTEGER NOT NULL REFERENCES reservation(id),
    host_id INTEGER NOT NULL REFERENCES host(id),
    image_id INTEGER REFERENCES image(id),
    state TEXT NOT NULL,
    ownership_marker TEXT NOT NULL,
    created_at TEXT NOT NULL,
    guest_ready_at TEXT,
    terminated_at TEXT
);

CREATE TABLE IF NOT EXISTS runner (
    id INTEGER PRIMARY KEY,
    runner_key TEXT NOT NULL UNIQUE,
    github_runner_id INTEGER,
    name TEXT NOT NULL,
    scale_set_id TEXT NOT NULL,
    scale_set_name TEXT NOT NULL,
    jit_state TEXT NOT NULL,
    jit_material_reference TEXT,
    profile_revision_id TEXT,
    vm_instance_id INTEGER REFERENCES vm_instance(id),
    created_at TEXT NOT NULL,
    registered_at TEXT,
    deregistered_at TEXT
);

CREATE TABLE IF NOT EXISTS workflow_run (
    id INTEGER PRIMARY KEY,
    installation_id INTEGER NOT NULL REFERENCES github_installation(id),
    repository TEXT NOT NULL,
    workflow_name TEXT NOT NULL,
    workflow_run_id INTEGER NOT NULL,
    head_branch TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (installation_id, workflow_run_id)
);

CREATE TABLE IF NOT EXISTS workflow_run_attempt (
    id INTEGER PRIMARY KEY,
    workflow_run_id INTEGER NOT NULL REFERENCES workflow_run(id),
    attempt INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (workflow_run_id, attempt)
);

CREATE TABLE IF NOT EXISTS job (
    id INTEGER PRIMARY KEY,
    workflow_run_attempt_id INTEGER NOT NULL REFERENCES workflow_run_attempt(id),
    github_job_key TEXT NOT NULL,
    rest_job_id INTEGER,
    protocol_job_id TEXT,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (workflow_run_attempt_id, github_job_key)
);

CREATE TABLE IF NOT EXISTS runner_request (
    id INTEGER PRIMARY KEY,
    job_id INTEGER NOT NULL REFERENCES job(id),
    request_id INTEGER NOT NULL,
    granted_at TEXT,
    expires_at TEXT,
    UNIQUE (job_id, request_id)
);

CREATE TABLE IF NOT EXISTS execution_attempt (
    id INTEGER PRIMARY KEY,
    job_id INTEGER NOT NULL REFERENCES job(id),
    attempt INTEGER NOT NULL,
    reservation_id INTEGER REFERENCES reservation(id),
    vm_instance_id INTEGER REFERENCES vm_instance(id),
    runner_id INTEGER REFERENCES runner(id),
    state TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    UNIQUE (job_id, attempt)
);

CREATE TABLE IF NOT EXISTS inbox (
    event_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    received_at TEXT NOT NULL,
    processed_at TEXT,
    decision_json TEXT
);

CREATE TABLE IF NOT EXISTS host_local_state (
    host_key TEXT PRIMARY KEY,
    state_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sample (
    id INTEGER PRIMARY KEY,
    host_id INTEGER REFERENCES host(id),
    execution_attempt_id INTEGER REFERENCES execution_attempt(id),
    vm_instance_id INTEGER REFERENCES vm_instance(id),
    scope TEXT NOT NULL,
    metric TEXT NOT NULL,
    value REAL NOT NULL,
    unit TEXT NOT NULL,
    monotonic_ns INTEGER,
    wall_ns INTEGER,
    sequence INTEGER NOT NULL,
    boot_id TEXT,
    collector_generation INTEGER,
    cohort INTEGER NOT NULL DEFAULT 1,
    captured_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS event (
    id INTEGER PRIMARY KEY,
    execution_attempt_id INTEGER REFERENCES execution_attempt(id),
    vm_instance_id INTEGER REFERENCES vm_instance(id),
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    occurred_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS timeline (
    id INTEGER PRIMARY KEY,
    execution_attempt_id INTEGER REFERENCES execution_attempt(id),
    phase TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    gap INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS log_reference (
    id INTEGER PRIMARY KEY,
    execution_attempt_id INTEGER REFERENCES execution_attempt(id),
    kind TEXT NOT NULL,
    scope TEXT NOT NULL,
    uri TEXT NOT NULL,
    captured_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS missing_data_observation (
    id INTEGER PRIMARY KEY,
    execution_attempt_id INTEGER REFERENCES execution_attempt(id),
    scope TEXT NOT NULL,
    metric TEXT NOT NULL,
    interval_start TEXT NOT NULL,
    interval_end TEXT NOT NULL,
    reason TEXT NOT NULL,
    observed_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_profile_revision_profile
    ON profile_revision (profile_name, revision_number);
CREATE INDEX IF NOT EXISTS idx_reservation_host_state
    ON reservation (host_id, state);
CREATE INDEX IF NOT EXISTS idx_vm_instance_reservation
    ON vm_instance (reservation_id);
CREATE INDEX IF NOT EXISTS idx_execution_attempt_job
    ON execution_attempt (job_id, attempt);
CREATE INDEX IF NOT EXISTS idx_sample_attempt_sequence
    ON sample (execution_attempt_id, sequence);
CREATE INDEX IF NOT EXISTS idx_sample_cohort
    ON sample (cohort);
CREATE INDEX IF NOT EXISTS idx_event_attempt_time
    ON event (execution_attempt_id, occurred_at);
CREATE INDEX IF NOT EXISTS idx_inbox_state
    ON inbox (state);

INSERT OR IGNORE INTO schema_migrations (version, applied_at)
    VALUES (1, '1970-01-01T00:00:00Z');
PRAGMA user_version = 1;
";

pub fn migrate(connection: &Connection) -> Result<()> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| map_error("begin migration", error))?;
    transaction
        .execute_batch(SCHEMA_V1)
        .map_err(|error| map_error("migrate schema v1", error))?;
    let user_version: u32 = transaction
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| map_error("read user_version", error))?;
    if user_version != SCHEMA_VERSION {
        return Err(StoreError::Migration(format!(
            "expected user_version {SCHEMA_VERSION}, found {user_version}"
        )));
    }
    transaction
        .commit()
        .map_err(|error| map_error("commit migration", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection::open_connection;
    use crate::store::test_support::{remove_db, temp_db_path};

    const EXPECTED_TABLES: &[&str] = &[
        "schema_migrations",
        "github_installation",
        "profile",
        "profile_revision",
        "image",
        "runtime_identity",
        "image_promotion",
        "image_channel",
        "host",
        "reservation",
        "vm_instance",
        "runner",
        "workflow_run",
        "workflow_run_attempt",
        "job",
        "runner_request",
        "execution_attempt",
        "inbox",
        "host_local_state",
        "sample",
        "event",
        "timeline",
        "log_reference",
        "missing_data_observation",
    ];

    fn table_names(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("prepare table query");
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query tables");
        rows.map(|row| row.expect("table name"))
            .filter(|name| !name.starts_with("sqlite_"))
            .collect()
    }

    #[test]
    fn store_migrations_create_every_persistent_entity_table() {
        let path = temp_db_path("migrations_tables");
        let connection = open_connection(&path).expect("open");
        migrate(&connection).expect("migrate schema v1");
        let found = table_names(&connection);
        for table in EXPECTED_TABLES {
            assert!(
                found.iter().any(|name| name == table),
                "missing table {table}"
            );
        }
        remove_db(&path);
    }

    #[test]
    fn store_migrations_are_idempotent() {
        let path = temp_db_path("migrations_idempotent");
        let connection = open_connection(&path).expect("open");
        migrate(&connection).expect("first migration");
        let before = table_names(&connection);
        migrate(&connection).expect("second migration");
        let after = table_names(&connection);
        assert_eq!(before, after);
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read user_version");
        assert_eq!(version, SCHEMA_VERSION);
        let rows: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| row.get(0))
            .expect("count migrations");
        assert_eq!(rows, 1);
        remove_db(&path);
    }

    #[test]
    fn store_profile_revisions_are_immutable() {
        let path = temp_db_path("migrations_immutable");
        let connection = open_connection(&path).expect("open");
        migrate(&connection).expect("migrate schema v1");
        connection
            .execute_batch(
                "INSERT INTO profile (name, current_revision, created_at)
                 VALUES ('balanced-8c16g', 1, '2026-11-21T00:00:00Z');
                 INSERT INTO profile_revision
                     (id, profile_name, revision_number, distro, release, arch,
                      vcpu, memory_gib, created_at)
                 VALUES ('abc', 'balanced-8c16g', 1, 'debian', '13', 'arm64',
                         8, 16, '2026-11-21T00:00:00Z');",
            )
            .expect("insert revision");
        let update = connection
            .execute(
                "UPDATE profile_revision SET memory_gib = 32 WHERE id = 'abc'",
                [],
            )
            .unwrap_err();
        assert!(update.to_string().contains("immutable"));
        let delete = connection
            .execute("DELETE FROM profile_revision WHERE id = 'abc'", [])
            .unwrap_err();
        assert!(delete.to_string().contains("immutable"));
        remove_db(&path);
    }
}
