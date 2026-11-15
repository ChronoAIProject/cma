//! Bounded format metadata I/O. All publication happens under the outer lock.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use super::hold_at;
use crate::failure::{Result, RuntimeError};

// Log only bounded classifications. Error Display can contain paths or caller
// payloads, and these diagnostics must never change the legacy wire failure.
fn io_error(
    predicate: &'static str,
    operation: &'static str,
    error: &std::io::Error,
) -> RuntimeError {
    tracing::error!(
        predicate,
        operation,
        error_kind = ?error.kind(),
        raw_os_error = ?error.raw_os_error(),
        "runtime mutation ledger format requires verification"
    );
    super::hold()
}

fn metadata_error(
    predicate: &'static str,
    operation: &'static str,
    metadata: &std::fs::Metadata,
) -> RuntimeError {
    tracing::error!(
        predicate,
        operation,
        mode_octal = %format_args!("{:04o}", metadata.mode() & 0o7777),
        group_other_bits = metadata.mode() & 0o077,
        required_group_other_bits = 0_u32,
        owner_read = metadata.mode() & 0o400 != 0,
        owner_write = metadata.mode() & 0o200 != 0,
        regular_file = metadata.is_file(),
        single_link = metadata.nlink() == 1,
        effective_owner = metadata.uid() == unsafe { libc::geteuid() },
        within_size_bound = metadata.len() <= 8192,
        "runtime mutation ledger format requires verification"
    );
    super::hold()
}

pub(crate) fn exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error("io.path_metadata", "path_metadata", &error)),
    }
}

pub(crate) fn directories(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if !std::fs::symlink_metadata(ancestor)
            .map_err(|error| io_error("io.ancestor_metadata", "ancestor_metadata", &error))?
            .is_dir()
        {
            return Err(hold_at("io.ancestor_directory"));
        }
    }
    Ok(())
}

fn open(path: &Path, create: bool, operation: &'static str) -> Result<File> {
    directories(path.parent().ok_or_else(|| hold_at("io.parent"))?)?;
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create_new(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| io_error("io.open", operation, &error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error("io.file_metadata", operation, &error))?;
    for (valid, predicate) in [
        (metadata.is_file(), "io.regular_file"),
        (metadata.nlink() == 1, "io.single_link"),
        (metadata.mode() & 0o077 == 0, "io.private_mode"),
        (
            metadata.uid() == unsafe { libc::geteuid() },
            "io.effective_owner",
        ),
        (metadata.len() <= 8192, "io.metadata_size"),
    ] {
        if !valid {
            return Err(metadata_error(predicate, operation, &metadata));
        }
    }
    Ok(file)
}

pub(crate) fn read(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open(path, false, "read_existing")?
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("io.read", "read_existing", &error))?;
    if bytes.len() > 8192 {
        return Err(hold_at("io.read_bound"));
    }
    Ok(bytes)
}

pub(super) fn create(path: &Path, bytes: &[u8]) -> Result<()> {
    if bytes.len() > 8192 {
        return Err(hold_at("io.write_bound"));
    }
    let mut file = open(path, true, "exclusive_create")?;
    file.write_all(bytes)
        .map_err(|error| io_error("io.write", "exclusive_create", &error))?;
    file.sync_all()
        .map_err(|error| io_error("io.file_sync", "exclusive_create", &error))?;
    crate::fs_durability::sync_parent(path)
        .map_err(|error| io_error("io.parent_sync", "exclusive_create", &error))
}

pub(super) fn lock(path: &Path, create: bool) -> Result<File> {
    let operation = if create {
        "exclusive_lock"
    } else {
        "lock_existing"
    };
    let file = open(path, create, operation)?;
    if file
        .metadata()
        .map_err(|error| io_error("io.lock_metadata", operation, &error))?
        .len()
        != 0
    {
        return Err(hold_at("io.lock_empty"));
    }
    file.try_lock().map_err(|_| {
        crate::failure::RuntimeError::rejected(
            "mutation_ledger_writer_held",
            "runtime mutation ledger has another writer",
        )
    })?;
    file.sync_all()
        .map_err(|error| io_error("io.lock_sync", operation, &error))?;
    crate::fs_durability::sync_parent(path)
        .map_err(|error| io_error("io.parent_sync", operation, &error))?;
    Ok(file)
}

pub(super) fn empty(path: &Path) -> Result<bool> {
    if !exists(path)? {
        return Ok(true);
    }
    directories(path)?;
    Ok(std::fs::read_dir(path)
        .map_err(|error| io_error("io.directory_inventory", "directory_inventory", &error))?
        .next()
        .is_none())
}

pub(super) fn make_directory(path: &Path) -> Result<()> {
    // Validate existing ancestors before create_dir_all can follow a link.
    let mut parent = path;
    while !exists(parent)? {
        parent = parent
            .parent()
            .ok_or_else(|| hold_at("io.creation_parent"))?;
    }
    directories(parent)?;
    crate::fs_durability::create_dir_all(path)
        .map_err(|error| io_error("io.create_directory", "create_directory", &error))?;
    directories(path)
}

#[cfg(test)]
#[path = "io_diagnostics_tests.rs"]
mod diagnostics_tests;

#[cfg(test)]
#[path = "io_recovery_diagnostics_tests.rs"]
mod recovery_diagnostics_tests;
