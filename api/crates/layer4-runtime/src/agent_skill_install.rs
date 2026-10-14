//! Add one skill without rewriting the home or replacing any existing entry.
use crate::{
    config::RuntimeConfig,
    failure::{Result, RuntimeError},
    paths::{path_is_directory, safe_component},
};
use layer3_adapter::{AgentSkillInstallReceipt, AgentSkillInstallRequest};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

#[cfg(test)]
#[path = "agent_skill_install_tests.rs"]
mod tests;

const MARKER: &str = ".cma-install-receipt.json";

pub(crate) fn validate(request: &AgentSkillInstallRequest) -> Result<()> {
    safe_component(request.agent_id.as_str(), "agent id")?;
    safe_component(&request.skill.id, "skill id")?;
    if matches!(request.skill.id.as_str(), "nyxid" | "ornn-agent-manual-cli")
        || !request.has_valid_sha256()
        || request.operation_id
            != format!(
                "agent:{}:skill:{}:install:v1",
                request.agent_id.as_str(),
                request.skill.id
            )
        || request
            .skill
            .version
            .as_ref()
            .is_none_or(|version| version.is_empty() || version.len() > 256)
        || request.skill.archive_sha256.len() != 64
        || !request
            .skill
            .archive_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(rejected("invalid immutable skill installation request"));
    }
    Ok(())
}

fn root(config: &RuntimeConfig, request: &AgentSkillInstallRequest) -> Result<PathBuf> {
    crate::persistent_home::verify(config)?;
    let root = config.agent_home(request.agent_id.as_str()).join("skills");
    let relative = root
        .strip_prefix(&config.home)
        .map_err(|_| rejected("skill path escaped runtime home"))?;
    let mut cursor = config.home.clone();
    if !path_is_directory(&cursor) {
        return Err(rejected("runtime home is not a real directory"));
    }
    for part in relative.components() {
        cursor.push(part);
        if !path_is_directory(&cursor) {
            return Err(rejected("skill parent is not a real directory"));
        }
    }
    Ok(root)
}

pub(crate) fn preflight(config: &RuntimeConfig, request: &AgentSkillInstallRequest) -> Result<()> {
    validate(request)?;
    let path = root(config, request)?.join(&request.skill.id);
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
        Ok(_) => Err(rejected("skill destination already exists")),
    }
}

pub(crate) fn recover(
    config: &RuntimeConfig,
    request: &AgentSkillInstallRequest,
) -> Result<Option<AgentSkillInstallReceipt>> {
    validate(request)?;
    let path = root(config, request)?.join(&request.skill.id);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
        Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => {}
        Ok(_) => return Err(rejected("skill destination is not a real directory")),
    }
    let marker = path.join(MARKER);
    let metadata = std::fs::symlink_metadata(&marker)
        .map_err(|_| rejected("existing skill has no matching install receipt"))?;
    if !metadata.is_file() || metadata.is_symlink() || metadata.len() > 4096 {
        return Err(rejected("invalid skill install receipt"));
    }
    let receipt: AgentSkillInstallReceipt =
        serde_json::from_slice(&std::fs::read(marker).map_err(io_error)?)
            .map_err(|_| rejected("invalid skill install receipt"))?;
    if !receipt.matches(request) {
        return Err(rejected("existing skill belongs to another installation"));
    }
    Ok(Some(receipt))
}

pub(crate) async fn install(
    config: &RuntimeConfig,
    client: &reqwest::Client,
    request: &AgentSkillInstallRequest,
) -> Result<AgentSkillInstallReceipt> {
    if let Some(receipt) = recover(config, request)? {
        return Ok(receipt);
    }
    let parent = root(config, request)?;
    let target = parent.join(&request.skill.id);
    let staging = parent.join(format!(".cma-skill-{}", request.request_sha256));
    match std::fs::symlink_metadata(&staging) {
        Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => {
            crate::fs_durability::remove_dir_all(&staging).map_err(io_error)?
        }
        Ok(_) => return Err(rejected("skill staging path is not a real directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    crate::skills::install_prepared(config, client, &request.skill, &staging).await?;
    // The archive cannot supply or overwrite our publication proof.
    let receipt = AgentSkillInstallReceipt::from_request(request);
    let mut marker = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(staging.join(MARKER))
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                rejected("archive contains a reserved installation receipt")
            } else {
                io_error(error)
            }
        })?;
    marker
        .write_all(&serde_json::to_vec(&receipt).expect("serializable receipt"))
        .map_err(io_error)?;
    marker.sync_all().map_err(io_error)?;
    crate::fs_durability::sync_tree(&staging).map_err(io_error)?;
    root(config, request)?;
    rename_new(&staging, &target).map_err(io_error)?;
    crate::fs_durability::sync_parent(&target).map_err(io_error)?;
    Ok(receipt)
}

// Publish atomically and refuse even an empty existing directory. A check followed
// by ordinary rename would silently overwrite a concurrent owner-created entry.
fn rename_new(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    let result = unsafe {
        // musl does not export renameat2 on every supported version. Invoke the
        // same atomic no-replace syscall using libc's architecture-specific ID.
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let result = -1;
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
fn rejected(detail: &str) -> RuntimeError {
    RuntimeError::rejected("agent_skill_install_conflict", detail)
}
fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable(
        "agent_skill_install_unavailable",
        format!("skill filesystem operation: {error}"),
    )
}
