//! Bounded, no-follow, append-only recovery evidence. Terminal files never
//! overwrite admission or the original Workspace preparation journal.
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use crate::failure::{Result, RuntimeError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

pub(super) fn directory(path: &Path) -> Result<Option<DirectoryIdentity>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            Ok(Some(DirectoryIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            }))
        }
        Ok(_) => Err(invalid("recovery path is not a real directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

pub(super) fn require_absent(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
        Ok(_) => Err(invalid(
            "recovery requires an absent publication or completion marker",
        )),
    }
}

pub(super) fn ensure_directory(path: &Path) -> Result<()> {
    if directory(path)?.is_some() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("recovery directory has no parent"))?;
    if directory(parent)?.is_none() {
        return Err(invalid("recovery parent directory is absent"));
    }
    crate::fs_durability::create_dir_all(path).map_err(io_error)?;
    directory(path)?.ok_or_else(|| invalid("recovery directory was not created"))?;
    Ok(())
}

pub(super) fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(invalid("recovery evidence is unsafe or oversized"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid("recovery evidence is oversized"));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| invalid("recovery evidence is malformed"))
}

pub(super) fn append<T: Serialize + DeserializeOwned + PartialEq>(
    path: &Path,
    value: &T,
) -> Result<()> {
    if let Some(existing) = read::<T>(path)? {
        return if &existing == value {
            Ok(())
        } else {
            Err(invalid(
                "recovery evidence conflicts with its immutable record",
            ))
        };
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("recovery evidence has no parent"))?;
    directory(parent)?.ok_or_else(|| invalid("recovery evidence directory is absent"))?;
    let bytes =
        serde_json::to_vec(value).map_err(|_| invalid("recovery evidence encoding failed"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid("recovery evidence is oversized"));
    }
    let temporary = parent.join(format!(".recovery-{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)
        .map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    // Link publication is atomic and never replaces an admitted/terminal file.
    // A crash leaves an ignorable temporary file, never a partially visible record.
    crate::fs_durability::hard_link(&temporary, path).map_err(io_error)?;
    crate::fs_durability::remove_file(&temporary).map_err(io_error)?;
    Ok(())
}

pub(super) fn invalid(detail: &str) -> RuntimeError {
    RuntimeError::rejected("workspace_recovery_ineligible", detail)
}
pub(super) fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable("workspace_recovery_io_failed", error.to_string())
}
