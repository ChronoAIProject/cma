use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};

pub(super) fn was_resumed(config: &RuntimeConfig, token: &str) -> Result<bool> {
    let Some(directory) = directory(config, false)? else {
        return Ok(false);
    };
    marker_exists(&directory.join(token))
}

/// Retire before resuming, so a delayed hold cannot recreate writer debt.
pub(super) fn retire(config: &RuntimeConfig, token: &str) -> Result<()> {
    retire_with(
        config,
        token,
        &mut File::sync_all,
        &mut crate::fs_durability::sync_directory,
    )
}

fn retire_with(
    config: &RuntimeConfig,
    token: &str,
    sync_file: &mut impl FnMut(&File) -> io::Result<()>,
    sync_directory: &mut impl FnMut(&Path) -> io::Result<()>,
) -> Result<()> {
    let directory = directory(config, true)?.expect("the token directory was created");
    let path = directory.join(token);
    match OpenOptions::new().create_new(true).write(true).open(&path) {
        Ok(file) => sync_file(&file).map_err(io_error)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !marker_exists(&path)? {
                return Err(invalid());
            }
            crate::fs_durability::sync_file_with(&path, sync_file).map_err(io_error)?;
        }
        Err(error) => return Err(io_error(error)),
    }
    crate::fs_durability::sync_directory_chain(&directory, sync_directory).map_err(io_error)
}

fn directory(config: &RuntimeConfig, create: bool) -> Result<Option<PathBuf>> {
    let mut path = config.checkpoint_root.clone();
    for component in [None, Some("checkpoint-writer-state")] {
        if let Some(component) = component {
            path.push(component);
        }
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(invalid()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                std::fs::create_dir_all(&path).map_err(io_error)?;
            }
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(Some(path))
}

fn marker_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() == 0 =>
        {
            Ok(true)
        }
        Ok(_) => Err(invalid()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "checkpoint_writer_state_invalid",
        "checkpoint writer token state is not a regular durable record",
    )
}

fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable(
        "checkpoint_writer_state_unavailable",
        format!("checkpoint writer state I/O failed: {:?}", error.kind()),
    )
}

#[cfg(test)]
#[path = "checkpoint_writer_tokens_tests.rs"]
mod tests;
