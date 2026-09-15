//! Removal writer ownership lives outside archived home bytes. A stable lock
//! serializes boot claim/publication with the final authority check and scrub.
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Writer {
    sandbox_id: String,
    home_identity: String,
    generation: u64,
    boot_id: String,
}

fn identity(config: &RuntimeConfig, boot_id: &str) -> Result<Writer> {
    Ok(Writer {
        sandbox_id: config.sandbox_id.clone(),
        home_identity: config.home_identity.clone().ok_or_else(invalid)?,
        generation: config.generation,
        boot_id: boot_id.into(),
    })
}

fn lock(config: &RuntimeConfig, create: bool) -> Result<File> {
    if create {
        crate::fs_durability::create_dir_all(&config.state_root).map_err(io_error)?;
    }
    if !crate::paths::path_is_directory(&config.state_root) {
        return Err(invalid());
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(config.state_root.join("workspace-removal-writer-v1.lock"))
        .map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() || metadata.len() != 0 {
        return Err(invalid());
    }
    file.try_lock().map_err(|_| {
        RuntimeError::unavailable(
            "workspace_removal_writer_busy",
            "Workspace removal writer is held by another operation",
        )
    })?;
    Ok(file)
}

fn record_path(config: &RuntimeConfig) -> std::path::PathBuf {
    config.state_root.join("workspace-removal-writer-v1.json")
}

fn read(config: &RuntimeConfig) -> Result<Option<Writer>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(record_path(config))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io_error)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| invalid())
}

/// Called only by supervisor construction, with a freshly generated boot ID.
/// Keep the returned lock through home generation initialization. Probes and
/// replay can only verify this record, so stale processes cannot reclaim it.
pub(crate) fn claim(config: &RuntimeConfig, boot_id: &str) -> Result<Option<File>> {
    if config.home_identity.is_none() {
        return Ok(None);
    }
    let guard = lock(config, true)?;
    let next = identity(config, boot_id)?;
    if read(config)?.is_some_and(|old| {
        old.sandbox_id != next.sandbox_id
            || old.home_identity != next.home_identity
            || old.generation > next.generation
    }) {
        return Err(invalid());
    }
    Ok(Some(guard))
}

/// The caller still holds claim's lock, and has initialized the home binding.
pub(crate) fn publish_claim(config: &RuntimeConfig, boot_id: &str) -> Result<()> {
    if config.home_identity.is_none() {
        return Ok(());
    }
    crate::persistent_home::verify(config)?.ok_or_else(invalid)?;
    let next = identity(config, boot_id)?;
    let temporary = config.state_root.join(format!(
        ".workspace-removal-writer-{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)
        .map_err(io_error)?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(&next).map_err(|_| invalid())?)
            .map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        crate::fs_durability::rename(&temporary, &record_path(config)).map_err(io_error)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    Ok(())
}

pub(crate) fn guard(config: &RuntimeConfig, boot_id: &str) -> Result<File> {
    let file = lock(config, false)?;
    if read(config)? != Some(identity(config, boot_id)?) {
        return Err(invalid());
    }
    if crate::persistent_home::verify(config)?.is_none() {
        return Err(invalid());
    }
    Ok(file)
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "workspace_removal_writer_stale",
        "this supervisor boot does not own current Workspace removal authority",
    )
}
fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable("workspace_removal_writer_unavailable", error.to_string())
}
