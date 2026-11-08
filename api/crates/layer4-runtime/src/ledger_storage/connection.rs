use std::fs::File;
use std::path::Path;
use std::time::Duration;

use rusqlite::{limits::Limit, Connection, OpenFlags};

use super::{invalid, paths, storage_error, MAX_VALUE_BYTES};
use crate::failure::Result;

pub(super) fn open(directory: &Path, create: bool, pages: u32) -> Result<(Connection, File)> {
    if pages < 16 || pages > i32::MAX as u32 {
        return Err(invalid());
    }
    let lock = paths::prepare(directory, create)?;
    let connection = Connection::open_with_flags(
        directory.join("ledger.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| storage_error())?;
    configure(&connection, pages)?;
    // Hot-journal recovery may run while SQLite first opens the schema. Establish
    // a file and directory barrier before any recovered state can authorize work.
    File::open(directory.join("ledger.sqlite3"))
        .and_then(|file| file.sync_all())
        .map_err(|_| storage_error())?;
    crate::fs_durability::sync_directory(directory).map_err(|_| storage_error())?;
    Ok((connection, lock))
}

fn configure(connection: &Connection, pages: u32) -> Result<()> {
    let configure = || -> rusqlite::Result<()> {
        connection.busy_timeout(Duration::from_millis(250))?;
        connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES)?;
        connection.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 64 * 1024)?;
        connection.set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 128)?;
        connection.pragma_update(None, "synchronous", "EXTRA")?;
        connection.pragma_update(None, "page_size", 4096)?;
        connection.pragma_update(None, "journal_mode", "DELETE")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "trusted_schema", false)?;
        connection.pragma_update(None, "secure_delete", true)?;
        connection.pragma_update(None, "max_page_count", pages)?;
        connection.pragma_update(None, "cache_size", -2048)?;
        connection.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(())
    };
    configure().map_err(|_| storage_error())?;
    verify(connection, pages)
}

pub(super) fn verify(connection: &Connection, pages: u32) -> Result<()> {
    let read_int = |name| {
        connection
            .pragma_query_value(None, name, |row| row.get::<_, i64>(0))
            .map_err(|_| storage_error())
    };
    let journal = connection
        .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
        .map_err(|_| storage_error())?;
    if journal != "delete"
        || read_int("synchronous")? != 3
        || read_int("foreign_keys")? != 1
        || read_int("trusted_schema")? != 0
        || read_int("secure_delete")? != 1
        || read_int("page_size")? != 4096
        || read_int("max_page_count")? != i64::from(pages)
        || read_int("cache_size")? != -2048
        || read_int("temp_store")? != 2
        || read_int("busy_timeout")? != 250
        || connection
            .limit(Limit::SQLITE_LIMIT_LENGTH)
            .map_err(|_| storage_error())?
            != MAX_VALUE_BYTES
    {
        return Err(invalid());
    }
    Ok(())
}
