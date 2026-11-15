use std::fs::{File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

use crate::failure::Result;

use super::{diagnostics, invalid, storage_error};

pub(super) fn prepare(directory: &Path, create: bool) -> Result<File> {
    let operation = if create {
        "create_store"
    } else {
        "open_existing_store"
    };
    for ancestor in directory.ancestors().skip(usize::from(create)) {
        let metadata = std::fs::symlink_metadata(ancestor).map_err(|error| {
            diagnostics::io(
                "storage.paths.ancestor_metadata",
                operation,
                "store_ancestor",
                &error,
                storage_error,
            )
        })?;
        if !metadata.is_dir() {
            return Err(diagnostics::metadata(
                "storage.paths.ancestor_directory",
                operation,
                "store_ancestor",
                &metadata,
            ));
        }
    }
    if create {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .map_err(|error| {
                diagnostics::io(
                    "storage.paths.create_directory",
                    operation,
                    "store_directory",
                    &error,
                    storage_error,
                )
            })?;
        crate::fs_durability::sync_directory(directory.parent().ok_or_else(invalid)?).map_err(
            |error| {
                diagnostics::io(
                    "storage.paths.parent_sync",
                    operation,
                    "store_parent",
                    &error,
                    storage_error,
                )
            },
        )?;
    }
    let metadata = std::fs::symlink_metadata(directory).map_err(|error| {
        diagnostics::io(
            "storage.paths.directory_metadata",
            operation,
            "store_directory",
            &error,
            storage_error,
        )
    })?;
    for (valid, predicate) in [
        (metadata.is_dir(), "storage.paths.directory_type"),
        (metadata.mode() & 0o077 == 0, "storage.paths.private_mode"),
        (
            metadata.uid() == unsafe { libc::geteuid() },
            "storage.paths.effective_owner",
        ),
    ] {
        if !valid {
            return Err(diagnostics::metadata(
                predicate,
                operation,
                "store_directory",
                &metadata,
            ));
        }
    }
    let lock = open_file(&directory.join("writer.lock"), create)?;
    if lock
        .metadata()
        .map_err(|error| {
            diagnostics::io(
                "storage.paths.lock_metadata",
                operation,
                "writer_lock",
                &error,
                storage_error,
            )
        })?
        .len()
        != 0
    {
        return Err(diagnostics::predicate(
            "storage.paths.lock_empty",
            operation,
            "writer_lock",
            invalid,
        ));
    }
    lock.try_lock()
        .map_err(|error| lock_error(error, operation))?;
    let database = open_file(&directory.join("ledger.sqlite3"), create)?;
    if create {
        database.sync_all().map_err(|error| {
            diagnostics::io(
                "storage.paths.database_sync",
                operation,
                "database",
                &error,
                storage_error,
            )
        })?;
        crate::fs_durability::sync_directory(directory).map_err(|error| {
            diagnostics::io(
                "storage.paths.directory_sync",
                operation,
                "store_directory",
                &error,
                storage_error,
            )
        })?;
    }
    // Refuse a symlink or nonregular journal before SQLite may recover it.
    let journal = directory.join("ledger.sqlite3-journal");
    match std::fs::symlink_metadata(&journal) {
        Ok(_) => {
            open_file(&journal, false)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(diagnostics::io(
                "storage.paths.journal_metadata",
                operation,
                "rollback_journal",
                &error,
                storage_error,
            ))
        }
    }
    for entry in std::fs::read_dir(directory).map_err(|error| {
        diagnostics::io(
            "storage.paths.inventory",
            operation,
            "store_directory",
            &error,
            storage_error,
        )
    })? {
        let entry = entry.map_err(|error| {
            diagnostics::io(
                "storage.paths.inventory_entry",
                operation,
                "store_directory",
                &error,
                storage_error,
            )
        })?;
        if !matches!(
            entry.file_name().to_str(),
            Some("writer.lock" | "ledger.sqlite3" | "ledger.sqlite3-journal" | "emergency.reserve")
        ) {
            return Err(diagnostics::predicate(
                "storage.paths.unknown_entry",
                operation,
                "store_directory",
                invalid,
            ));
        }
    }
    super::reserve::prepare(directory, create)?;
    Ok(lock)
}

fn writer_held() -> crate::failure::RuntimeError {
    crate::failure::RuntimeError::rejected(
        "mutation_ledger_writer_held",
        "runtime mutation ledger has another writer",
    )
}

fn lock_error(
    error: std::fs::TryLockError,
    operation: &'static str,
) -> crate::failure::RuntimeError {
    match error {
        std::fs::TryLockError::WouldBlock => diagnostics::predicate(
            "storage.paths.writer_busy",
            operation,
            "writer_lock",
            writer_held,
        ),
        std::fs::TryLockError::Error(error) => diagnostics::io(
            "storage.paths.lock_syscall",
            operation,
            "writer_lock",
            &error,
            writer_held,
        ),
    }
}

pub(super) fn open_file(path: &Path, create: bool) -> Result<File> {
    let operation = if create {
        "exclusive_create"
    } else {
        "open_existing"
    };
    let role = match path.file_name().and_then(|name| name.to_str()) {
        Some("writer.lock") => "writer_lock",
        Some("ledger.sqlite3") => "database",
        Some("ledger.sqlite3-journal") => "rollback_journal",
        Some("emergency.reserve") => "emergency_reserve",
        _ => "store_file",
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            diagnostics::io("storage.file.open", operation, role, &error, storage_error)
        })?;
    let metadata = file.metadata().map_err(|error| {
        diagnostics::io(
            "storage.file.metadata",
            operation,
            role,
            &error,
            storage_error,
        )
    })?;
    for (valid, predicate) in [
        (metadata.is_file(), "storage.file.regular_file"),
        (metadata.nlink() == 1, "storage.file.single_link"),
        (metadata.mode() & 0o077 == 0, "storage.file.private_mode"),
        (
            metadata.uid() == unsafe { libc::geteuid() },
            "storage.file.effective_owner",
        ),
    ] {
        if !valid {
            return Err(diagnostics::metadata(predicate, operation, role, &metadata));
        }
    }
    Ok(file)
}
