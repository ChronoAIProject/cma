//! Bounded format metadata I/O. All publication happens under the outer lock.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use super::hold_at;
use crate::failure::Result;

pub(crate) fn exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(hold_at("io.path_metadata")),
    }
}

pub(crate) fn directories(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if !std::fs::symlink_metadata(ancestor)
            .map_err(|_| hold_at("io.ancestor_metadata"))?
            .is_dir()
        {
            return Err(hold_at("io.ancestor_directory"));
        }
    }
    Ok(())
}

fn open(path: &Path, create: bool) -> Result<File> {
    directories(path.parent().ok_or_else(|| hold_at("io.parent"))?)?;
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create_new(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| hold_at("io.open"))?;
    let metadata = file.metadata().map_err(|_| hold_at("io.file_metadata"))?;
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
            return Err(hold_at(predicate));
        }
    }
    Ok(file)
}

pub(crate) fn read(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open(path, false)?
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| hold_at("io.read"))?;
    if bytes.len() > 8192 {
        return Err(hold_at("io.read_bound"));
    }
    Ok(bytes)
}

pub(super) fn create(path: &Path, bytes: &[u8]) -> Result<()> {
    if bytes.len() > 8192 {
        return Err(hold_at("io.write_bound"));
    }
    let mut file = open(path, true)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| hold_at("io.write_sync"))?;
    crate::fs_durability::sync_parent(path).map_err(|_| hold_at("io.parent_sync"))
}

pub(super) fn lock(path: &Path, create: bool) -> Result<File> {
    let file = open(path, create)?;
    if file
        .metadata()
        .map_err(|_| hold_at("io.lock_metadata"))?
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
    file.sync_all().map_err(|_| hold_at("io.lock_sync"))?;
    crate::fs_durability::sync_parent(path).map_err(|_| hold_at("io.parent_sync"))?;
    Ok(file)
}

pub(super) fn empty(path: &Path) -> Result<bool> {
    if !exists(path)? {
        return Ok(true);
    }
    directories(path)?;
    Ok(std::fs::read_dir(path)
        .map_err(|_| hold_at("io.directory_inventory"))?
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
    crate::fs_durability::create_dir_all(path).map_err(|_| hold_at("io.create_directory"))?;
    directories(path)
}
