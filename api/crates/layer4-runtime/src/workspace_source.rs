use std::ffi::OsString;
use std::path::Path;

#[cfg(test)]
use std::path::PathBuf;

use layer3_adapter::{WorkspaceReceipt, WorkspaceRequest, WorkspaceSource};

use crate::failure::{Result, RuntimeError};
use crate::git::GitRunner;
use crate::paths::{path_is_directory, safe_component, validate_selector};

pub(crate) fn validate_clone_depth(depth: u32) -> Result<()> {
    if !(1..=1_000).contains(&depth) {
        return Err(RuntimeError::rejected(
            "invalid_clone_depth",
            "clone depth must be between 1 and 1000",
        ));
    }
    Ok(())
}

pub(crate) async fn prepare(
    home: &Path,
    allow_local_git: bool,
    request: &WorkspaceRequest,
) -> Result<WorkspaceReceipt> {
    validate_clone_depth(request.clone_depth)?;
    safe_component(&request.workspace_id, "workspace id")?;
    safe_component(&request.name, "workspace name")?;
    ensure_home(home)?;
    let path = home.join(&request.name);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(RuntimeError::rejected(
                "workspace_exists",
                "workspace path already exists",
            ))
        }
        Err(error) => return Err(io_error("inspect workspace path", error)),
    }

    let git = GitRunner::new(allow_local_git);
    let resolved_commit = async {
        match &request.source {
            WorkspaceSource::RepositorySnapshot { .. }
            | WorkspaceSource::PublicRepositorySnapshot { .. } => {
                return Err(RuntimeError::rejected(
                    "repository_connection_required",
                    "source snapshots require the credentialed workspace preparation path",
                ))
            }
            WorkspaceSource::Clone { url, reference } => {
                clone_workspace(
                    home,
                    &path,
                    url,
                    reference.as_deref(),
                    request.clone_depth,
                    &git,
                    allow_local_git,
                )
                .await?;
            }
            WorkspaceSource::Init => init_workspace(&path, &git).await?,
        }
        install_worktree_metadata(&path, &request.workspace_id)?;
        resolve_commit(&path, &git, "HEAD").await
    }
    .await?;
    crate::fs_durability::sync_tree(&path)
        .map_err(|error| io_error("sync prepared workspace", error))?;
    Ok(WorkspaceReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace_id: request.workspace_id.clone(),
        name: request.name.clone(),
        path: path.to_string_lossy().into_owned(),
        resolved_commit,
        source_commit: None,
    })
}

/// Recover a preparation that completed before its response was durably
/// recorded. Partial or non-Git directories deliberately fail closed.
pub(crate) async fn inspect_prepared(
    home: &Path,
    allow_local_git: bool,
    request: &WorkspaceRequest,
) -> Result<Option<WorkspaceReceipt>> {
    validate_clone_depth(request.clone_depth)?;
    safe_component(&request.workspace_id, "workspace id")?;
    safe_component(&request.name, "workspace name")?;
    let path = home.join(&request.name);
    if !path.exists() {
        return Ok(None);
    }
    if !path_is_directory(&path) || !path_is_directory(&path.join(".git")) {
        return Err(RuntimeError::rejected(
            "workspace_prepare_indeterminate",
            "workspace preparation stopped with a partial directory",
        ));
    }
    require_workspace_identity(&path, &request.workspace_id)?;
    let resolved_commit = resolve_commit(&path, &GitRunner::new(allow_local_git), "HEAD").await?;
    crate::fs_durability::sync_tree(&path)
        .map_err(|error| io_error("sync recovered workspace", error))?;
    Ok(Some(WorkspaceReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace_id: request.workspace_id.clone(),
        name: request.name.clone(),
        path: path.to_string_lossy().into_owned(),
        resolved_commit,
        source_commit: crate::repository_snapshot::source_commit(&path, &request.source)?,
    }))
}

pub(crate) fn remove(home: &Path, workspace_id: &str, name: &str) -> Result<()> {
    safe_component(workspace_id, "workspace id")?;
    safe_component(name, "workspace name")?;
    let path = home.join(name);
    let staging = home.join(".cma").join("preparing").join(workspace_id);
    let quarantine = home.join(".cma").join("removing");
    let tombstone = quarantine.join(workspace_id);
    if path.exists() && staging.exists() {
        return Err(RuntimeError::rejected(
            "workspace_incarnation_ambiguous",
            "workspace has both published and staged directories",
        ));
    }
    if !tombstone.exists() && path.exists() {
        if !path_is_directory(&path.join(".git")) {
            return Err(RuntimeError::rejected(
                "workspace_invalid",
                "workspace is not a main Git checkout",
            ));
        }
        require_workspace_identity(&path, workspace_id)?;
        crate::fs_durability::create_dir_all(&quarantine)
            .map_err(|error| io_error("create workspace removal quarantine", error))?;
        crate::fs_durability::rename(&path, &tombstone)
            .map_err(|error| io_error("quarantine workspace", error))?;
    }
    if !tombstone.exists() && staging.exists() {
        crate::fs_durability::create_dir_all(&quarantine)
            .map_err(|error| io_error("create workspace removal quarantine", error))?;
        crate::fs_durability::rename(&staging, &tombstone)
            .map_err(|error| io_error("quarantine staged workspace", error))?;
    }
    if tombstone.exists() {
        crate::fs_durability::remove_dir_all(&tombstone)
            .map_err(|error| io_error("remove quarantined workspace", error))?;
    }
    Ok(())
}

pub(crate) async fn resolve_commit(
    repository: &Path,
    git: &GitRunner,
    revision: &str,
) -> Result<String> {
    validate_selector(revision, "revision")?;
    let selector = format!("{revision}^{{commit}}");
    let output = git
        .checked(
            repository,
            ["rev-parse", "--verify", selector.as_str()],
            "resolve commit",
        )
        .await?;
    let commit = output.stdout.trim();
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RuntimeError::rejected(
            "git_invalid_commit",
            "Git returned an invalid commit object ID",
        ));
    }
    Ok(commit.to_ascii_lowercase())
}

async fn clone_workspace(
    home: &Path,
    path: &Path,
    url: &str,
    reference: Option<&str>,
    clone_depth: u32,
    git: &GitRunner,
    allow_local_git: bool,
) -> Result<()> {
    validate_clone_url(url, allow_local_git)?;
    if let Some(reference) = reference {
        validate_selector(reference, "clone reference")?;
    }
    let mut args = vec![
        OsString::from("clone"),
        OsString::from("--depth"),
        OsString::from(clone_depth.to_string()),
    ];
    if let Some(reference) = reference {
        args.extend([
            OsString::from("--branch"),
            OsString::from(reference),
            OsString::from("--single-branch"),
        ]);
    }
    args.extend([
        OsString::from("--"),
        OsString::from(url),
        path.as_os_str().to_owned(),
    ]);
    git.checked_clone(home, args).await?;
    Ok(())
}

async fn init_workspace(path: &Path, git: &GitRunner) -> Result<()> {
    crate::fs_durability::create_dir_all(path)
        .map_err(|error| io_error("create workspace", error))?;
    git.checked(path, ["init", "-b", "main", "."], "init workspace")
        .await?;
    crate::fs_durability::write(&path.join(".gitkeep"), &[])
        .map_err(|error| io_error("write .gitkeep", error))?;
    git.checked(path, ["add", "--", ".gitkeep"], "stage initial workspace")
        .await?;
    git.checked(
        path,
        [
            "-c",
            "user.name=CMA",
            "-c",
            "user.email=noreply@chrono-ai.fun",
            "commit",
            "-m",
            "Initial workspace",
        ],
        "commit initial workspace",
    )
    .await?;
    Ok(())
}

pub(crate) fn validate_clone_url(value: &str, allow_local: bool) -> Result<()> {
    if allow_local && (Path::new(value).is_absolute() || value.starts_with("file://")) {
        return Ok(());
    }
    let url = url::Url::parse(value).map_err(|_| {
        RuntimeError::rejected(
            "invalid_clone_url",
            "clone URL is not a valid public GitHub URL",
        )
    })?;
    let segments = url
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || github_authority(value) != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || segments.len() != 2
    {
        return Err(RuntimeError::rejected(
            "invalid_clone_url",
            "clone URL must name one public HTTPS GitHub repository",
        ));
    }
    Ok(())
}

fn github_authority(value: &str) -> Option<&str> {
    value
        .strip_prefix("https://")?
        .split(['/', '?', '#'])
        .next()
}

pub(crate) fn install_worktree_metadata(repository: &Path, workspace_id: &str) -> Result<()> {
    let info = repository.join(".git/info");
    crate::fs_durability::create_dir_all(&info)
        .map_err(|error| io_error("create Git info directory", error))?;
    let exclude = info.join("exclude");
    let mut contents = match std::fs::read_to_string(&exclude) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(io_error("read Git excludes", error)),
    };
    if !contents.lines().any(|line| line.trim() == ".worktrees/") {
        if !contents.is_empty() && !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str(".worktrees/\n");
        crate::fs_durability::write(&exclude, contents.as_bytes())
            .map_err(|error| io_error("write Git excludes", error))?;
    }
    crate::fs_durability::write(&info.join("cma-workspace-id"), workspace_id.as_bytes())
        .map_err(|error| io_error("write workspace identity", error))?;
    Ok(())
}

pub(crate) fn require_workspace_identity(repository: &Path, workspace_id: &str) -> Result<()> {
    let stored = std::fs::read_to_string(repository.join(".git/info/cma-workspace-id"))
        .map_err(|error| io_error("read workspace identity", error))?;
    if stored != workspace_id {
        return Err(RuntimeError::rejected(
            "workspace_incarnation_mismatch",
            "workspace path belongs to another workspace incarnation",
        ));
    }
    Ok(())
}

fn ensure_home(home: &Path) -> Result<()> {
    crate::fs_durability::create_dir_all(home)
        .map_err(|error| io_error("create sandbox home", error))?;
    if !path_is_directory(home) {
        return Err(RuntimeError::rejected(
            "home_invalid",
            "sandbox home is not a directory",
        ));
    }
    Ok(())
}

fn io_error(operation: &'static str, error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable("workspace_io_failed", format!("{operation}: {error}"))
}

#[cfg(test)]
#[path = "workspace_source_tests.rs"]
mod tests;
