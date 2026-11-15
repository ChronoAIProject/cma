//! Inspect a closed, small fresh store without SQLite recovery or metadata writes.
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rusqlite::{limits::Limit, Connection, OpenFlags};

use super::{diagnostics, invalid, paths, reserve, storage_error};
use crate::failure::Result;

pub(crate) struct ClosedInspection {
    inodes: [(u64, u64); 3],
    _writer: File,
}

impl ClosedInspection {
    pub(crate) fn verify_at(&self, directory: &Path) -> Result<()> {
        if identities(directory)? != self.inodes {
            return Err(diagnostics::predicate(
                "storage.inspection.identity",
                "inspect_closed",
                "store",
                invalid,
            ));
        }
        Ok(())
    }
}

fn identities(directory: &Path) -> Result<[(u64, u64); 3]> {
    for ancestor in directory.ancestors() {
        if !std::fs::symlink_metadata(ancestor)
            .map_err(|error| {
                diagnostics::io(
                    "storage.inspection.ancestor_metadata",
                    "inspect_closed",
                    "store_ancestor",
                    &error,
                    invalid,
                )
            })?
            .is_dir()
        {
            return Err(diagnostics::predicate(
                "storage.inspection.ancestor_directory",
                "inspect_closed",
                "store_ancestor",
                invalid,
            ));
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
        let role = ["store_directory", "database", "writer_lock"][index];
        let metadata = std::fs::symlink_metadata(path).map_err(|error| {
            diagnostics::io(
                "storage.inspection.path_metadata",
                "inspect_closed",
                role,
                &error,
                invalid,
            )
        })?;
        for (valid, predicate) in [
            (
                index != 0 || metadata.is_dir(),
                "storage.inspection.directory_type",
            ),
            (
                index == 0 || metadata.is_file(),
                "storage.inspection.regular_file",
            ),
            (
                index == 0 || metadata.nlink() == 1,
                "storage.inspection.single_link",
            ),
            (
                metadata.mode() & 0o077 == 0,
                "storage.inspection.private_mode",
            ),
            (
                metadata.uid() == unsafe { libc::geteuid() },
                "storage.inspection.effective_owner",
            ),
        ] {
            if !valid {
                return Err(diagnostics::metadata(
                    predicate,
                    "inspect_closed",
                    role,
                    &metadata,
                ));
            }
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
    for (name, role) in [
        ("ledger.sqlite3-journal", "rollback_journal"),
        ("ledger.sqlite3-wal", "wal"),
        ("ledger.sqlite3-shm", "shared_memory"),
    ] {
        match std::fs::symlink_metadata(directory.join(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(diagnostics::predicate(
                    "storage.inspection.journal_present",
                    "inspect_closed",
                    role,
                    invalid,
                ))
            }
            Err(error) => {
                return Err(diagnostics::io(
                    "storage.inspection.journal_metadata",
                    "inspect_closed",
                    role,
                    &error,
                    invalid,
                ))
            }
        }
    }
    let lock = paths::prepare(directory, false)?;
    let metadata = lock.metadata().map_err(|error| {
        diagnostics::io(
            "storage.inspection.lock_metadata",
            "inspect_closed",
            "writer_lock",
            &error,
            invalid,
        )
    })?;
    if (metadata.dev(), metadata.ino()) != inodes[2] {
        return Err(diagnostics::predicate(
            "storage.inspection.lock_identity",
            "inspect_closed",
            "writer_lock",
            invalid,
        ));
    }
    reserve::require(directory)?;
    // This path is only for the initial empty schema, never arbitrary retained DBs.
    let database = directory.join("ledger.sqlite3");
    if std::fs::symlink_metadata(&database)
        .map_err(|error| {
            diagnostics::io(
                "storage.inspection.database_metadata",
                "inspect_closed",
                "database",
                &error,
                storage_error,
            )
        })?
        .len()
        > 128 * 4096
    {
        return Err(diagnostics::predicate(
            "storage.inspection.database_size",
            "inspect_closed",
            "database",
            invalid,
        ));
    }
    let db = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|error| diagnostics::sqlite("storage.inspection.open", &error, storage_error))?;
    db.busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|error| {
            diagnostics::sqlite("storage.inspection.busy_timeout", &error, storage_error)
        })?;
    db.set_limit(Limit::SQLITE_LIMIT_LENGTH, super::MAX_VALUE_BYTES)
        .map_err(|error| {
            diagnostics::sqlite("storage.inspection.value_limit", &error, storage_error)
        })?;
    db.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 64 * 1024)
        .map_err(|error| {
            diagnostics::sqlite("storage.inspection.sql_limit", &error, storage_error)
        })?;
    db.pragma_update(None, "trusted_schema", false)
        .map_err(|error| {
            diagnostics::sqlite("storage.inspection.trusted_schema", &error, storage_error)
        })?;
    let integrity: String = db
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|error| diagnostics::sqlite("storage.inspection.quick_check", &error, invalid))?;
    if integrity != "ok" {
        return Err(diagnostics::predicate(
            "storage.inspection.integrity",
            "inspect_closed",
            "database",
            invalid,
        ));
    }
    inspect(&db)?;
    db.close().map_err(|(_, error)| {
        diagnostics::sqlite("storage.inspection.close", &error, storage_error)
    })?;
    // Bind the inspected pathname and held lock before the caller may publish.
    let proof = ClosedInspection {
        inodes,
        _writer: lock,
    };
    proof.verify_at(directory)?;
    Ok(proof)
}
