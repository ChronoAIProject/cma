//! Bounded format metadata I/O. All publication happens under the outer lock.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use super::hold;
use crate::failure::Result;

pub(super) fn exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(hold()),
    }
}

pub(super) fn directories(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if !std::fs::symlink_metadata(ancestor)
            .map_err(|_| hold())?
            .is_dir()
        {
            return Err(hold());
        }
    }
    Ok(())
}

fn open(path: &Path, create: bool) -> Result<File> {
    directories(path.parent().ok_or_else(hold)?)?;
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create_new(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| hold())?;
    let metadata = file.metadata().map_err(|_| hold())?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.len() > 8192
    {
        return Err(hold());
    }
    Ok(file)
}

pub(super) fn read(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open(path, false)?
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| hold())?;
    if bytes.len() > 8192 {
        return Err(hold());
    }
    Ok(bytes)
}

pub(super) fn create(path: &Path, bytes: &[u8]) -> Result<()> {
    if bytes.len() > 8192 {
        return Err(hold());
    }
    let mut file = open(path, true)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| hold())?;
    crate::fs_durability::sync_parent(path).map_err(|_| hold())
}

pub(super) fn lock(path: &Path, create: bool) -> Result<File> {
    let file = open(path, create)?;
    if file.metadata().map_err(|_| hold())?.len() != 0 {
        return Err(hold());
    }
    file.try_lock().map_err(|_| {
        crate::failure::RuntimeError::rejected(
            "mutation_ledger_writer_held",
            "runtime mutation ledger has another writer",
        )
    })?;
    file.sync_all().map_err(|_| hold())?;
    crate::fs_durability::sync_parent(path).map_err(|_| hold())?;
    Ok(file)
}

pub(super) fn empty(path: &Path) -> Result<bool> {
    if !exists(path)? {
        return Ok(true);
    }
    directories(path)?;
    Ok(std::fs::read_dir(path)
        .map_err(|_| hold())?
        .next()
        .is_none())
}

pub(super) fn make_directory(path: &Path) -> Result<()> {
    // Validate existing ancestors before create_dir_all can follow a link.
    let mut parent = path;
    while !exists(parent)? {
        parent = parent.parent().ok_or_else(hold)?;
    }
    directories(parent)?;
    crate::fs_durability::create_dir_all(path).map_err(|_| hold())?;
    directories(path)
}
