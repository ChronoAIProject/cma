//! Refuse ambiguous or unsafe restored paths before any reconciliation effect.
use std::path::Path;

use crate::failure::{Result, RuntimeError};
use crate::paths::{path_is_directory, safe_component};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presence {
    Absent,
    Original,
    Successor,
}

pub(crate) fn inspect(home: &Path, workspace_id: &str, name: &str) -> Result<Presence> {
    safe_component(workspace_id, "workspace id")?;
    safe_component(name, "workspace name")?;
    if !path_is_directory(home) {
        return Err(unsafe_path());
    }
    let cma = home.join(".cma");
    directory_if_present(&cma)?;
    let preparing = cma.join("preparing");
    let removing = cma.join("removing");
    directory_if_present(&preparing)?;
    directory_if_present(&removing)?;
    let paths = [
        home.join(name),
        preparing.join(workspace_id),
        removing.join(workspace_id),
    ];
    let mut found = None;
    for (index, path) in paths.iter().enumerate() {
        if !directory_if_present(path)? {
            continue;
        }
        if found.is_some() {
            return Err(RuntimeError::rejected(
                "workspace_incarnation_ambiguous",
                "multiple published, staged or quarantined Workspace directories exist",
            ));
        }
        for directory in [path.join(".git"), path.join(".git/info")] {
            if !path_is_directory(&directory) {
                return Err(unsafe_path());
            }
        }
        let marker = path.join(".git/info/cma-workspace-id");
        let metadata = std::fs::symlink_metadata(&marker).map_err(|_| unsafe_path())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 {
            return Err(unsafe_path());
        }
        let identity = std::fs::read_to_string(marker).map_err(|_| unsafe_path())?;
        safe_component(&identity, "Workspace identity marker")?;
        found = Some(if identity == workspace_id {
            Presence::Original
        } else if index == 0 {
            Presence::Successor
        } else {
            return Err(unsafe_path());
        });
    }
    Ok(found.unwrap_or(Presence::Absent))
}

fn directory_if_present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        _ => Err(unsafe_path()),
    }
}

fn unsafe_path() -> RuntimeError {
    RuntimeError::rejected(
        "workspace_removal_path_unsafe",
        "Workspace removal requires exact real directories and a regular immutable identity marker",
    )
}
