use std::fs::{File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

use crate::failure::Result;

use super::{invalid, storage_error};

pub(super) fn prepare(directory: &Path, create: bool) -> Result<File> {
    for ancestor in directory.ancestors().skip(usize::from(create)) {
        let metadata = std::fs::symlink_metadata(ancestor).map_err(|_| storage_error())?;
        if !metadata.is_dir() {
            return Err(invalid());
        }
    }
    if create {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .map_err(|_| storage_error())?;
        crate::fs_durability::sync_directory(directory.parent().ok_or_else(invalid)?)
            .map_err(|_| storage_error())?;
    }
    let metadata = std::fs::symlink_metadata(directory).map_err(|_| storage_error())?;
    if !metadata.is_dir()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(invalid());
    }
    let lock = open_file(&directory.join("writer.lock"), create)?;
    if lock.metadata().map_err(|_| storage_error())?.len() != 0 {
        return Err(invalid());
    }
    lock.try_lock().map_err(|_| {
        crate::failure::RuntimeError::rejected(
            "mutation_ledger_writer_held",
            "runtime mutation ledger has another writer",
        )
    })?;
    let database = open_file(&directory.join("ledger.sqlite3"), create)?;
    if create {
        database.sync_all().map_err(|_| storage_error())?;
        crate::fs_durability::sync_directory(directory).map_err(|_| storage_error())?;
    }
    // Refuse a symlink or nonregular journal before SQLite may recover it.
    let journal = directory.join("ledger.sqlite3-journal");
    match std::fs::symlink_metadata(&journal) {
        Ok(_) => {
            open_file(&journal, false)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(storage_error()),
    }
    for entry in std::fs::read_dir(directory).map_err(|_| storage_error())? {
        let entry = entry.map_err(|_| storage_error())?;
        if !matches!(
            entry.file_name().to_str(),
            Some("writer.lock" | "ledger.sqlite3" | "ledger.sqlite3-journal" | "emergency.reserve")
        ) {
            return Err(invalid());
        }
    }
    super::reserve::prepare(directory, create)?;
    Ok(lock)
}

pub(super) fn open_file(path: &Path, create: bool) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| storage_error())?;
    let metadata = file.metadata().map_err(|_| storage_error())?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(invalid());
    }
    Ok(file)
}
