//! Inspect a closed, small fresh store without SQLite recovery or metadata writes.
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rusqlite::{limits::Limit, Connection, OpenFlags};

use super::{invalid, paths, reserve, storage_error};
use crate::failure::Result;

pub(crate) struct ClosedInspection {
    inodes: [(u64, u64); 3],
    _writer: File,
}

impl ClosedInspection {
    pub(crate) fn verify_at(&self, directory: &Path) -> Result<()> {
        if identities(directory)? != self.inodes {
            return Err(invalid());
        }
        Ok(())
    }
}

fn identities(directory: &Path) -> Result<[(u64, u64); 3]> {
    for ancestor in directory.ancestors() {
        if !std::fs::symlink_metadata(ancestor)
            .map_err(|_| invalid())?
            .is_dir()
        {
            return Err(invalid());
        }
    }
    let mut identities = [(0, 0); 3];
    for (index, path) in [
        directory.to_owned(),
        directory.join("ledger.sqlite3"),
        directory.join("writer.lock"),
    ]
    .iter()
    .enumerate()
    {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| invalid())?;
        if (index == 0 && !metadata.is_dir())
            || (index != 0 && (!metadata.is_file() || metadata.nlink() != 1))
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(invalid());
        }
        identities[index] = (metadata.dev(), metadata.ino());
    }
    Ok(identities)
}

pub(crate) fn inspect_closed(
    directory: &Path,
    inspect: impl FnOnce(&Connection) -> Result<()>,
) -> Result<ClosedInspection> {
    let inodes = identities(directory)?;
    // A completed fresh store has no journal. Never use immutable=1 to hide one.
    for name in [
        "ledger.sqlite3-journal",
        "ledger.sqlite3-wal",
        "ledger.sqlite3-shm",
    ] {
        match std::fs::symlink_metadata(directory.join(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(invalid()),
        }
    }
    let lock = paths::prepare(directory, false)?;
    let metadata = lock.metadata().map_err(|_| invalid())?;
    if (metadata.dev(), metadata.ino()) != inodes[2] {
        return Err(invalid());
    }
    reserve::require(directory)?;
    // This path is only for the initial empty schema, never arbitrary retained DBs.
    let database = directory.join("ledger.sqlite3");
    if std::fs::symlink_metadata(&database)
        .map_err(|_| storage_error())?
        .len()
        > 128 * 4096
    {
        return Err(invalid());
    }
    let db = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| storage_error())?;
    db.busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|_| storage_error())?;
    db.set_limit(Limit::SQLITE_LIMIT_LENGTH, super::MAX_VALUE_BYTES)
        .map_err(|_| storage_error())?;
    db.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 64 * 1024)
        .map_err(|_| storage_error())?;
    db.pragma_update(None, "trusted_schema", false)
        .map_err(|_| storage_error())?;
    let integrity: String = db
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|_| invalid())?;
    if integrity != "ok" {
        return Err(invalid());
    }
    inspect(&db)?;
    db.close().map_err(|_| storage_error())?;
    // Bind the inspected pathname and held lock before the caller may publish.
    let proof = ClosedInspection {
        inodes,
        _writer: lock,
    };
    proof.verify_at(directory)?;
    Ok(proof)
}
