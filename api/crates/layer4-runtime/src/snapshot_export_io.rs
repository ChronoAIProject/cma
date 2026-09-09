//! Read-only source checks and exact, bounded export records.

use std::fs::OpenOptions;
use std::io::{Read as _, Write as _};
use std::path::Path;

use serde::{de::DeserializeOwned, Serialize};

use crate::failure::{Result, RuntimeError};

const MAX_RECORD_BYTES: u64 = 256 * 1024;

pub(super) fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failure(error)),
    };
    let metadata = file.metadata().map_err(failure)?;
    if !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES {
        return Err(invalid("export record is not a bounded regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid("export record exceeds its size bound"));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| invalid("export record is malformed"))
}

/// Read a Kubernetes projected file (for example a DownwardAPI annotation).
/// Atomic projected volumes expose the file through an in-volume `..data`
/// symlink. Resolve that provider-created link, require the result to remain
/// under the projected mount, then pass the canonical path through the normal
/// no-follow reader so user-controlled links are still rejected.
pub(super) fn read_projected<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("projected record has no parent"))?;
    let parent = std::fs::canonicalize(parent).map_err(failure)?;
    let canonical = std::fs::canonicalize(path).map_err(failure)?;
    if !canonical.starts_with(&parent) {
        return Err(invalid("projected record resolves outside its mount"));
    }
    read(&canonical)
}

pub(super) fn publish<T: Serialize + DeserializeOwned + PartialEq>(
    path: &Path,
    value: &T,
) -> Result<()> {
    if let Some(existing) = read::<T>(path)? {
        return if &existing == value {
            Ok(())
        } else {
            Err(invalid("export record already names different authority"))
        };
    }
    let bytes =
        serde_json::to_vec(value).map_err(|_| invalid("export record cannot be encoded"))?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid("export record exceeds its size bound"));
    }
    let temporary = path.with_file_name(format!(".export-{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(failure)?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(failure)?;
        drop(file);
        match crate::fs_durability::hard_link(&temporary, path) {
            Ok(()) => Ok(()),
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    && read::<T>(path)?.as_ref() == Some(value) =>
            {
                Ok(())
            }
            Err(error) => Err(failure(error)),
        }
    })();
    let cleanup = crate::fs_durability::remove_file(&temporary).map_err(failure);
    result.and(cleanup)
}

pub(super) fn directory(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || !crate::paths::path_is_directory(path)
        || std::fs::canonicalize(path).map_err(failure)? != path
    {
        return Err(invalid(
            "export directories must be canonical real directories",
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn read_only(path: &Path) -> Result<bool> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(failure)?;
    let mut status = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // fstatvfs initializes the complete output on success, and the descriptor
    // remains owned by directory until after both the syscall and inspection.
    let result = unsafe { libc::fstatvfs(directory.as_raw_fd(), status.as_mut_ptr()) };
    if result != 0 {
        return Err(failure(std::io::Error::last_os_error()));
    }
    let status = unsafe { status.assume_init() };
    Ok(status.f_flag & libc::ST_RDONLY != 0)
}

#[cfg(not(unix))]
pub(super) fn read_only(_path: &Path) -> Result<bool> {
    Err(invalid(
        "read-only snapshot mounts are unavailable on this platform",
    ))
}

pub(super) fn invalid(detail: &'static str) -> RuntimeError {
    RuntimeError::rejected("snapshot_export_invalid", detail)
}

fn failure(error: std::io::Error) -> RuntimeError {
    // Source paths can contain user-controlled file names; keep command logs bounded.
    // The error kind is sufficient to diagnose provider mount failures without
    // exposing paths or file contents from the captured home.
    RuntimeError::unavailable(
        "snapshot_export_io",
        format!(
            "snapshot export storage operation failed ({:?})",
            error.kind()
        ),
    )
}
