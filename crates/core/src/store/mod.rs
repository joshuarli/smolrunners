mod backup;
mod connection;
mod error;
mod migrations;
mod telemetry;
mod txn;

pub use backup::backup_to;
pub use connection::{
    MIN_SQLITE_VERSION, Store, assert_sqlite_version, open_connection, verify_effective_pragmas,
    version_string_at_least,
};
pub use error::{Result, StoreError};
pub use migrations::{SCHEMA_VERSION, SCHEMA_V1, migrate};
pub use telemetry::{SampleRow, TelemetryWriter};
pub use txn::{DEFAULT_READ_ROW_LIMIT, ReadTxn};

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn temp_db_path(label: &str) -> PathBuf {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "smolrunners-store-{label}-{}-{sequence}.sqlite",
            std::process::id()
        ))
    }

    pub(crate) fn remove_db(path: &Path) {
        let name = path
            .file_name()
            .expect("database file name")
            .to_string_lossy()
            .into_owned();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut candidate = path.to_path_buf();
            if !suffix.is_empty() {
                candidate.set_file_name(format!("{name}{suffix}"));
            }
            let _ = std::fs::remove_file(candidate);
        }
    }
}
