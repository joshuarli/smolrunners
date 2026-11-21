use std::fmt;

use rusqlite::ErrorCode;

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, PartialEq)]
pub enum StoreError {
    PragmaMismatch {
        name: &'static str,
        expected: String,
        actual: String,
    },
    SqliteVersionTooOld {
        found: String,
        required: &'static str,
    },
    UnrecognizedSqliteVersion(String),
    Busy {
        operation: &'static str,
    },
    DiskFull {
        operation: &'static str,
    },
    ReadBudgetExceeded {
        limit: u64,
    },
    Migration(String),
    Sqlite(rusqlite::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PragmaMismatch {
                name,
                expected,
                actual,
            } => write!(
                f,
                "effective {name} pragma is {actual}; required {expected}"
            ),
            Self::SqliteVersionTooOld { found, required } => {
                write!(f, "SQLite {found} is older than required {required}")
            }
            Self::UnrecognizedSqliteVersion(version) => {
                write!(f, "unrecognized SQLite version {version:?}")
            }
            Self::Busy { operation } => write!(f, "database busy during {operation}"),
            Self::DiskFull { operation } => write!(f, "disk full during {operation}"),
            Self::ReadBudgetExceeded { limit } => {
                write!(f, "read transaction exceeded its {limit} row budget")
            }
            Self::Migration(message) => write!(f, "migration failed: {message}"),
            Self::Sqlite(error) => write!(f, "SQLite error: {error}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

pub(crate) fn map_error(operation: &'static str, error: rusqlite::Error) -> StoreError {
    match &error {
        rusqlite::Error::SqliteFailure(ffi_error, _)
            if ffi_error.code == ErrorCode::DatabaseBusy =>
        {
            StoreError::Busy { operation }
        }
        rusqlite::Error::SqliteFailure(ffi_error, _) if ffi_error.code == ErrorCode::DiskFull => {
            StoreError::DiskFull { operation }
        }
        _ => StoreError::Sqlite(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqlite_error(code: i32) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None)
    }

    #[test]
    fn store_maps_sqlite_busy_to_an_explicit_error() {
        let mapped = map_error("reserve", sqlite_error(rusqlite::ffi::SQLITE_BUSY));
        assert_eq!(mapped, StoreError::Busy { operation: "reserve" });
        assert!(mapped.to_string().contains("busy"));
    }

    #[test]
    fn store_maps_disk_full_to_an_explicit_error() {
        let mapped = map_error("flush", sqlite_error(rusqlite::ffi::SQLITE_FULL));
        assert_eq!(mapped, StoreError::DiskFull { operation: "flush" });
        assert!(mapped.to_string().contains("disk full"));
    }

    #[test]
    fn store_reports_busy_when_another_writer_holds_the_lock() {
        use crate::store::connection::{Store, open_connection};
        use crate::store::test_support::{remove_db, temp_db_path};

        let path = temp_db_path("busy_writer");
        let first = Store::open(&path).expect("open first store");
        first
            .connection()
            .execute_batch("CREATE TABLE contention (value INTEGER NOT NULL);")
            .expect("create table");
        first
            .connection()
            .execute_batch("PRAGMA busy_timeout = 0;")
            .expect("disable waiting on first connection");
        let second = open_connection(&path).expect("open second connection");
        second
            .execute_batch("PRAGMA busy_timeout = 0;")
            .expect("disable waiting on second connection");

        let write = first
            .connection()
            .unchecked_transaction()
            .expect("begin write transaction");
        write
            .execute("INSERT INTO contention VALUES (1)", [])
            .expect("insert while holding the write lock");
        let error = second
            .execute("INSERT INTO contention VALUES (2)", [])
            .expect_err("second writer must be busy");
        assert_eq!(
            map_error("reserve", error),
            StoreError::Busy { operation: "reserve" }
        );
        drop(write);
        remove_db(&path);
    }

    #[test]
    fn store_preserves_other_sqlite_errors() {
        let mapped = map_error("query", sqlite_error(rusqlite::ffi::SQLITE_CORRUPT));
        assert_eq!(
            mapped,
            StoreError::Sqlite(sqlite_error(rusqlite::ffi::SQLITE_CORRUPT))
        );
    }
}
