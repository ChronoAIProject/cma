use std::fs::OpenOptions;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use super::{invalid, Binding};
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};

const DIRECTORY: &str = "home-binding";
const CANONICAL: &str = "binding.json";
const MAX_BYTES: u64 = 4096;

fn directory(config: &RuntimeConfig) -> PathBuf {
    config.state_root.join(DIRECTORY)
}

fn check_directory(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(invalid("home binding directory is not a real directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(unavailable(error)),
    }
}

pub(super) fn read(config: &RuntimeConfig) -> Result<Option<Binding>> {
    if !check_directory(&config.state_root)? || !check_directory(&directory(config))? {
        return Ok(None);
    }
    read_file(&directory(config).join(CANONICAL))
}

fn read_file(path: &Path) -> Result<Option<Binding>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unavailable(error)),
    };
    let metadata = file.metadata().map_err(unavailable)?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(invalid("home binding is not a bounded regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unavailable)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid("home binding exceeds its size bound"));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| invalid("home binding is malformed"))
}

pub(super) fn uninitialized(config: &RuntimeConfig) -> Result<bool> {
    if !check_directory(&config.state_root)? {
        return Ok(true);
    }
    for entry in std::fs::read_dir(&config.state_root).map_err(unavailable)? {
        if entry.map_err(unavailable)?.file_name() != DIRECTORY {
            return Ok(false);
        }
    }
    if check_directory(&directory(config))? {
        // Temporary writes are never authority, including a partial write at
        // process loss. Only a published binding can precede installation.
        temporary_files(config)?;
    }
    Ok(true)
}

pub(super) fn publish(config: &RuntimeConfig, record: &Binding, initial: bool) -> Result<()> {
    record.validate(config)?;
    let root = directory(config);
    check_directory(&config.state_root)?;
    check_directory(&root)?;
    crate::fs_durability::create_dir_all(&root).map_err(unavailable)?;
    let temporary = root.join(format!(".home-{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(unavailable)?;
    let bytes =
        serde_json::to_vec(record).map_err(|_| invalid("home binding cannot be serialized"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(unavailable)?;
    drop(file);
    let canonical = root.join(CANONICAL);
    if initial {
        // Publish without replacing any concurrently established ownership.
        if let Err(error) = crate::fs_durability::hard_link(&temporary, &canonical) {
            if error.kind() != std::io::ErrorKind::AlreadyExists
                || read(config)?.as_ref() != Some(record)
            {
                return Err(unavailable(error));
            }
        }
        crate::fs_durability::remove_file(&temporary).map_err(unavailable)
    } else {
        crate::fs_durability::rename(&temporary, &canonical).map_err(unavailable)
    }
}

fn temporary_files(config: &RuntimeConfig) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for (count, entry) in std::fs::read_dir(directory(config))
        .map_err(unavailable)?
        .enumerate()
    {
        if count >= 32 {
            return Err(invalid("home binding directory has too many entries"));
        }
        let entry = entry.map_err(unavailable)?;
        let name = entry.file_name();
        if name == CANONICAL {
            continue;
        }
        let name = name
            .to_str()
            .ok_or_else(|| invalid("home binding filename is invalid"))?;
        let id = name
            .strip_prefix(".home-")
            .and_then(|s| s.strip_suffix(".tmp"));
        if !id.is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err(invalid("home binding directory has an unknown entry"));
        }
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(unavailable)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_BYTES {
            return Err(invalid(
                "unpublished home binding is not a bounded regular file",
            ));
        }
        paths.push(entry.path());
    }
    Ok(paths)
}

pub(super) fn clean_temporary(config: &RuntimeConfig) -> Result<()> {
    for path in temporary_files(config)? {
        crate::fs_durability::remove_file(&path).map_err(unavailable)?;
    }
    Ok(())
}

fn unavailable(error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable(
        "persistent_home_unavailable",
        format!("home binding I/O: {error}"),
    )
}
