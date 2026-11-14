use super::*;
use std::io::{Read, Write};
use std::path::Path;

const MARKER: &[u8] = b"cma-desktop-owner-v1\n";

pub(super) fn load(root: &Path) -> Result<Option<Record>> {
    let marker = root.with_extension("activated");
    match std::fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return match std::fs::symlink_metadata(marker) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                _ => Err(unavailable()),
            };
        }
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        _ => return Err(unavailable()),
    }
    // Even an empty namespace is an interrupted activation, never legacy.
    if read(&marker)? != MARKER {
        return Err(unavailable());
    }
    let record: Record =
        serde_json::from_slice(&read(&root.join("floor.json"))?).map_err(|_| unavailable())?;
    if record.version != 1
        || record.owner.validate().is_err()
        || record.receiver.sandbox != record.owner.sandbox_id
        || record.receiver.generation == 0
        || record.receiver.boot.is_empty()
        || record
            .owner
            .boot_id
            .as_ref()
            .is_some_and(|boot| boot != &record.receiver.boot)
    {
        return Err(unavailable());
    }
    Ok(Some(record))
}

fn read(path: &Path) -> Result<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path).map_err(|_| unavailable())?;
    if !file.metadata().map_err(|_| unavailable())?.is_file() {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    file.take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() > 8192 {
        return Err(unavailable());
    }
    Ok(bytes)
}

pub(super) fn save(root: &Path, record: &Record) -> Result<()> {
    crate::fs_durability::create_dir_all(root).map_err(|_| unavailable())?;
    let metadata = std::fs::symlink_metadata(root).map_err(|_| unavailable())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(unavailable());
    }
    // Keep activation outside the floor directory so losing that directory
    // cannot restore legacy admission. Both are part of the reader floor.
    let marker = root.with_extension("activated");
    match std::fs::symlink_metadata(&marker) {
        Ok(_) if read(&marker)? == MARKER => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_synced_file(&marker, MARKER)?;
            crate::fs_durability::sync_parent(&marker).map_err(|_| unavailable())?;
        }
        _ => return Err(unavailable()),
    }
    let temporary = root.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        write_synced_file(
            &temporary,
            &serde_json::to_vec(record).map_err(|_| unavailable())?,
        )?;
        // The temporary name need not survive a crash: until replacement the
        // old floor remains authoritative. Sync its complete contents first,
        // then atomically replace and sync the destination's directory entry.
        crate::fs_durability::rename(&temporary, &root.join("floor.json"))
            .map_err(|_| unavailable())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn write_synced_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options.open(path).map_err(|_| unavailable())?;
    file.write_all(bytes).map_err(|_| unavailable())?;
    file.sync_all().map_err(|_| unavailable())
}
