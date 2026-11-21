use std::path::Path;

use rusqlite::Connection;

use crate::store::error::{Result, StoreError, map_error};
use crate::store::migrations::migrate;

pub const MIN_SQLITE_VERSION: &str = "3.51.3";

const REQUIRED_SYNCHRONOUS: i64 = 2;
const REQUIRED_BUSY_TIMEOUT_MS: i64 = 5000;

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = open_connection(path)?;
        migrate(&connection)?;
        Ok(Self { connection })
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }
}

pub fn open_connection(path: impl AsRef<Path>) -> Result<Connection> {
    let connection = Connection::open(path).map_err(|error| map_error("open database", error))?;
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|error| map_error("configure pragmas", error))?;
    assert_sqlite_version(&connection)?;
    verify_effective_pragmas(&connection)?;
    Ok(connection)
}

pub fn assert_sqlite_version(connection: &Connection) -> Result<()> {
    let version: String = connection
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .map_err(|error| map_error("read sqlite_version", error))?;
    if version_string_at_least(&version, MIN_SQLITE_VERSION)? {
        Ok(())
    } else {
        Err(StoreError::SqliteVersionTooOld {
            found: version,
            required: MIN_SQLITE_VERSION,
        })
    }
}

pub fn verify_effective_pragmas(connection: &Connection) -> Result<()> {
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(|error| map_error("read journal_mode", error))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(StoreError::PragmaMismatch {
            name: "journal_mode",
            expected: "wal".to_owned(),
            actual: journal_mode,
        });
    }
    let synchronous: i64 = connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .map_err(|error| map_error("read synchronous", error))?;
    if synchronous != REQUIRED_SYNCHRONOUS {
        return Err(StoreError::PragmaMismatch {
            name: "synchronous",
            expected: "FULL (2)".to_owned(),
            actual: synchronous.to_string(),
        });
    }
    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(|error| map_error("read foreign_keys", error))?;
    if foreign_keys != 1 {
        return Err(StoreError::PragmaMismatch {
            name: "foreign_keys",
            expected: "ON (1)".to_owned(),
            actual: foreign_keys.to_string(),
        });
    }
    let busy_timeout: i64 = connection
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .map_err(|error| map_error("read busy_timeout", error))?;
    if busy_timeout != REQUIRED_BUSY_TIMEOUT_MS {
        return Err(StoreError::PragmaMismatch {
            name: "busy_timeout",
            expected: REQUIRED_BUSY_TIMEOUT_MS.to_string(),
            actual: busy_timeout.to_string(),
        });
    }
    Ok(())
}

pub fn version_string_at_least(version: &str, minimum: &str) -> Result<bool> {
    let parsed_version = parse_version(version)
        .ok_or_else(|| StoreError::UnrecognizedSqliteVersion(version.to_owned()))?;
    let parsed_minimum = parse_version(minimum)
        .ok_or_else(|| StoreError::UnrecognizedSqliteVersion(minimum.to_owned()))?;
    Ok(parsed_version >= parsed_minimum)
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::error::StoreError;
    use crate::store::test_support::{remove_db, temp_db_path};

    #[test]
    fn store_open_rejects_a_synchronous_normal_connection() {
        let path = temp_db_path("pragma_mismatch");
        let connection = Connection::open(&path).expect("open raw connection");
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
            .expect("set synchronous NORMAL");
        let error = verify_effective_pragmas(&connection).expect_err("pragma mismatch");
        assert_eq!(
            error,
            StoreError::PragmaMismatch {
                name: "synchronous",
                expected: "FULL (2)".to_owned(),
                actual: "1".to_owned(),
            }
        );
        remove_db(&path);
    }

    #[test]
    fn store_open_enforces_effective_pragmas_on_every_connection() {
        let path = temp_db_path("pragma_enforced");
        {
            let raw = Connection::open(&path).expect("open raw connection");
            raw.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
                .expect("set synchronous NORMAL");
        }
        let store = Store::open(&path).expect("open repairs pragmas");
        verify_effective_pragmas(store.connection()).expect("effective pragmas");
        let synchronous: i64 = store
            .connection()
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("read synchronous");
        assert_eq!(synchronous, 2);
        remove_db(&path);
    }

    #[test]
    fn store_checks_sqlite_version_before_opening() {
        let path = temp_db_path("version_checked");
        let connection = open_connection(&path).expect("open");
        assert_sqlite_version(&connection).expect("bundled SQLite meets the minimum");
        let reported: String = connection
            .query_row("SELECT sqlite_version()", [], |row| row.get(0))
            .expect("read version");
        assert!(version_string_at_least(&reported, MIN_SQLITE_VERSION).expect("parse version"));
        remove_db(&path);
    }

    #[test]
    fn store_version_comparison_rejects_older_and_unrecognized_versions() {
        assert!(version_string_at_least("3.54.1", MIN_SQLITE_VERSION).expect("newer"));
        assert!(version_string_at_least("3.51.3", MIN_SQLITE_VERSION).expect("equal"));
        assert!(!version_string_at_least("3.51.2", MIN_SQLITE_VERSION).expect("older"));
        assert_eq!(
            version_string_at_least("not-a-version", MIN_SQLITE_VERSION),
            Err(StoreError::UnrecognizedSqliteVersion(
                "not-a-version".to_owned()
            ))
        );
    }
}
