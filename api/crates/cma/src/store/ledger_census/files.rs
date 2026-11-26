use super::{Result, MAX_REQUEST, OUTPUT};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

pub(in crate::store) fn require_parent(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(OUTPUT);
    }
    let parent = path.parent().ok_or(OUTPUT)?;
    let meta = fs::symlink_metadata(parent).map_err(|_| OUTPUT)?;
    if !meta.is_dir() || meta.mode() & 0o777 != 0o700 {
        return Err(OUTPUT);
    }
    Ok(())
}

pub(in crate::store) fn read(path: &Path) -> Result<Vec<u8>> {
    require_parent(path)?;
    let before = fs::symlink_metadata(path).map_err(|_| OUTPUT)?;
    if !before.is_file()
        || before.mode() & 0o777 != 0o600
        || before.nlink() != 1
        || before.len() > MAX_REQUEST
    {
        return Err(OUTPUT);
    }
    let file = fs::File::open(path).map_err(|_| OUTPUT)?;
    let opened = file.metadata().map_err(|_| OUTPUT)?;
    if (before.dev(), before.ino()) != (opened.dev(), opened.ino()) {
        return Err(OUTPUT);
    }
    let mut bytes = Vec::new();
    file.take(MAX_REQUEST + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| OUTPUT)?;
    if bytes.len() as u64 > MAX_REQUEST {
        return Err(OUTPUT);
    }
    Ok(bytes)
}

pub(in crate::store) fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    require_parent(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| OUTPUT)?;
    let outcome = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::File::open(path.parent().unwrap())?.sync_all());
    if outcome.is_err() {
        let _ = file.set_len(0);
        let _ = file.sync_all();
        return Err(OUTPUT);
    }
    Ok(())
}
