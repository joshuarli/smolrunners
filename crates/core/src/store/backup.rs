use std::ffi::CStr;
use std::path::Path;
use std::thread;
use std::time::Duration;

use rusqlite::Connection;
use rusqlite::ffi;

use crate::store::error::{Result, map_error};

const MAX_BACKUP_RETRIES: u32 = 60_000;

pub fn backup_to(source: &Connection, destination: impl AsRef<Path>) -> Result<()> {
    let destination =
        Connection::open(destination).map_err(|error| map_error("open backup destination", error))?;
    let source_handle = unsafe { source.handle() };
    let destination_handle = unsafe { destination.handle() };
    let backup = unsafe {
        ffi::sqlite3_backup_init(
            destination_handle,
            c"main".as_ptr(),
            source_handle,
            c"main".as_ptr(),
        )
    };
    if backup.is_null() {
        return Err(map_error(
            "initialize backup",
            database_error(destination_handle, ffi::SQLITE_ERROR),
        ));
    }

    let mut retries = 0u32;
    let step_code = loop {
        let code = unsafe { ffi::sqlite3_backup_step(backup, 256) };
        if code == ffi::SQLITE_DONE || code == ffi::SQLITE_OK {
            break code;
        }
        if code != ffi::SQLITE_BUSY && code != ffi::SQLITE_LOCKED {
            break code;
        }
        retries += 1;
        if retries > MAX_BACKUP_RETRIES {
            break code;
        }
        thread::sleep(Duration::from_millis(1));
    };
    let finish_code = unsafe { ffi::sqlite3_backup_finish(backup) };

    if step_code != ffi::SQLITE_DONE && step_code != ffi::SQLITE_OK {
        return Err(map_error(
            "run backup",
            database_error(destination_handle, step_code),
        ));
    }
    if finish_code != ffi::SQLITE_OK {
        return Err(map_error(
            "finish backup",
            database_error(destination_handle, finish_code),
        ));
    }
    Ok(())
}

fn database_error(handle: *mut ffi::sqlite3, code: i32) -> rusqlite::Error {
    let message = unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(handle)) }
        .to_string_lossy()
        .into_owned();
    rusqlite::Error::SqliteFailure(ffi::Error::new(code), Some(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection::{Store, open_connection};
    use crate::store::test_support::{remove_db, temp_db_path};

    #[test]
    fn store_backup_captures_a_consistent_snapshot() {
        let path = temp_db_path("backup_source");
        let destination = temp_db_path("backup_destination");
        let source = Store::open(&path).expect("open source");
        source
            .connection()
            .execute(
                "INSERT INTO host
                    (host_key, incarnation, hostname, arch, total_vcpu,
                     total_memory_gib, total_disk_bytes, capabilities_json,
                     admission_state, created_at)
                 VALUES ('host-1', 1, 'node-a', 'arm64', 8, 16, 1000000, '{}', 'eligible', 'now')",
                [],
            )
            .expect("insert host");
        backup_to(source.connection(), &destination).expect("backup");

        let restored = open_connection(&destination).expect("open backup");
        let count: i64 = restored
            .query_row("SELECT COUNT(*) FROM host", [], |row| row.get(0))
            .expect("count hosts");
        assert_eq!(count, 1);
        let host_key: String = restored
            .query_row("SELECT host_key FROM host", [], |row| row.get(0))
            .expect("read host");
        assert_eq!(host_key, "host-1");

        source
            .connection()
            .execute(
                "INSERT INTO host
                    (host_key, incarnation, hostname, arch, total_vcpu,
                     total_memory_gib, total_disk_bytes, capabilities_json,
                     admission_state, created_at)
                 VALUES ('host-2', 1, 'node-b', 'arm64', 8, 16, 1000000, '{}', 'eligible', 'now')",
                [],
            )
            .expect("insert second host");
        let restored_count: i64 = restored
            .query_row("SELECT COUNT(*) FROM host", [], |row| row.get(0))
            .expect("count hosts after source change");
        assert_eq!(restored_count, 1);

        remove_db(&path);
        remove_db(&destination);
    }
}
