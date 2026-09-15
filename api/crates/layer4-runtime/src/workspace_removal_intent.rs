//! A current-observation removal must never become an uncredentialed startup
//! recovery effect. The canonical legacy journal remains the prepare barrier.
use crate::{
    config::RuntimeConfig,
    failure::{Result, RuntimeError},
};
use layer3_adapter::WorkspaceRemoveRequest;
use sha2::{Digest, Sha256};

fn path(config: &RuntimeConfig, request: &WorkspaceRemoveRequest) -> std::path::PathBuf {
    config
        .state_root
        .join("workspace-removal-current-v1")
        .join(format!(
            "{}.json",
            crate::command::hex(&Sha256::digest(request.operation_id.as_bytes()))
        ))
}

pub(crate) fn requires_current(
    config: &RuntimeConfig,
    request: &WorkspaceRemoveRequest,
) -> Result<bool> {
    let path = path(config, request);
    let parent = path.parent().expect("intent directory");
    match std::fs::symlink_metadata(parent) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        _ => return Err(invalid()),
    }
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= 4096 => {}
        _ => return Err(invalid()),
    }
    let saved: WorkspaceRemoveRequest =
        serde_json::from_slice(&std::fs::read(path).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
    if saved != *request || !saved.has_valid_sha256() {
        return Err(invalid());
    }
    Ok(true)
}

pub(crate) fn record(config: &RuntimeConfig, request: &WorkspaceRemoveRequest) -> Result<()> {
    if requires_current(config, request)? {
        return Ok(());
    }
    let path = path(config, request);
    crate::fs_durability::create_dir_all(path.parent().expect("intent directory"))
        .map_err(|_| invalid())?;
    let bytes = serde_json::to_vec(request).map_err(|_| invalid())?;
    crate::fs_durability::write(&path, &bytes).map_err(|_| invalid())
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "workspace_removal_intent_invalid",
        "current Workspace removal intent is missing, unsafe or inconsistent",
    )
}
