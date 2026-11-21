use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use layer3_adapter::{WorktreeReceipt, WorktreeRef, WorktreeRequest};

use crate::failure::{Result, RuntimeError};
use crate::git::GitRunner;
use crate::paths::{path_is_directory, safe_component, validate_selector};
use crate::workspace_source::resolve_commit;

pub(crate) async fn add(
    home: &Path,
    allow_local_git: bool,
    request: &WorktreeRequest,
) -> Result<WorktreeReceipt> {
    preflight_add(home, allow_local_git, request).await?;
    let repository = home.join(&request.workspace);
    let parent = repository.join(".worktrees");
    let path = parent.join(&request.name);
    let git = GitRunner::new(allow_local_git);
    let base_commit = resolve_commit(&repository, &git, &request.base).await?;
    let branch = branch_name(&request.name);
    let args = vec![
        OsString::from("worktree"),
        OsString::from("add"),
        OsString::from("-b"),
        OsString::from(&branch),
        path.as_os_str().to_owned(),
        OsString::from(&base_commit),
    ];
    if let Err(error) = git
        .checked_checkout(&repository, args, "add worktree")
        .await
    {
        cleanup_failed_add(&git, &repository, &path, &branch).await;
        return Err(error);
    }
    crate::fs_durability::sync_tree(&repository.join(".git")).map_err(worktree_io(
        "sync repository metadata after adding worktree",
    ))?;
    crate::fs_durability::sync_tree(&path).map_err(worktree_io("sync added worktree"))?;
    Ok(WorktreeReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace: request.workspace.clone(),
        name: request.name.clone(),
        path: path.to_string_lossy().into_owned(),
        branch,
        base_commit,
    })
}

pub(crate) async fn preflight_add(
    home: &Path,
    allow_local_git: bool,
    request: &WorktreeRequest,
) -> Result<()> {
    validate_add_request(request)?;
    safe_component(&request.workspace, "workspace name")?;
    safe_component(&request.name, "worktree name")?;
    validate_selector(&request.base, "worktree base")?;
    let repository = home.join(&request.workspace);
    require_repository(&repository)?;
    let parent = repository.join(".worktrees");
    ensure_worktree_parent(&parent)?;
    if entry_exists(
        &parent.join(&request.name),
        "inspect existing worktree path",
    )? {
        return Err(RuntimeError::rejected(
            "worktree_exists",
            "worktree path already exists",
        ));
    }
    let git = GitRunner::new(allow_local_git);
    resolve_commit(&repository, &git, &request.base).await?;
    if branch_exists(&git, &repository, &branch_name(&request.name)).await? {
        return Err(RuntimeError::rejected(
            "worktree_exists",
            "worktree branch already exists",
        ));
    }
    Ok(())
}

pub(crate) async fn inspect_add(
    home: &Path,
    allow_local_git: bool,
    request: &WorktreeRequest,
) -> Result<Option<WorktreeReceipt>> {
    validate_add_request(request)?;
    safe_component(&request.workspace, "workspace name")?;
    safe_component(&request.name, "worktree name")?;
    validate_selector(&request.base, "worktree base")?;
    let repository = home.join(&request.workspace);
    require_repository(&repository)?;
    let path = expected_path(home, &request.workspace, &request.name);
    let path_exists = real_directory_or_absent(&path)?;
    let git = GitRunner::new(allow_local_git);
    let branch = branch_name(&request.name);
    let branch_exists = branch_exists(&git, &repository, &branch).await?;
    match (path_exists, branch_exists) {
        (false, false) => return Ok(None),
        (true, true) => {}
        _ => {
            return Err(add_indeterminate(
                "worktree path and branch were only partly created",
            ))
        }
    }
    if !real_file(&path.join(".git"))? {
        return Err(add_indeterminate(
            "recovered worktree does not have a Git link file",
        ));
    }
    let expected_commit = resolve_commit(&repository, &git, &request.base).await?;
    let branch_commit = resolve_commit(&repository, &git, &branch).await?;
    let worktree_commit = resolve_commit(&path, &git, "HEAD").await?;
    let actual_branch = symbolic_branch(&git, &path).await?;
    if branch_commit != expected_commit
        || worktree_commit != expected_commit
        || actual_branch != branch
    {
        return Err(add_indeterminate(
            "recovered worktree branch or commit differs from the admitted addition",
        ));
    }
    Ok(Some(WorktreeReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace: request.workspace.clone(),
        name: request.name.clone(),
        path: path.to_string_lossy().into_owned(),
        branch,
        base_commit: expected_commit,
    }))
}

async fn branch_exists(git: &GitRunner, repository: &Path, branch: &str) -> Result<bool> {
    let reference = format!("refs/heads/{branch}");
    let output = git
        .checked(
            repository,
            ["for-each-ref", "--format=%(refname)", reference.as_str()],
            "inspect worktree branch",
        )
        .await?;
    Ok(output.stdout.lines().any(|line| line == reference))
}

async fn cleanup_failed_add(git: &GitRunner, repository: &Path, path: &Path, branch: &str) {
    let _ = git
        .checked(
            repository,
            [
                OsString::from("worktree"),
                OsString::from("remove"),
                OsString::from("--force"),
                path.as_os_str().to_owned(),
            ],
            "clean failed worktree",
        )
        .await;
    let _ = crate::fs_durability::remove_dir_all(path);
    let _ = git
        .checked(
            repository,
            ["worktree", "prune", "--expire", "now"],
            "prune failed worktree",
        )
        .await;
    let _ = git
        .checked(
            repository,
            ["branch", "-D", branch],
            "clean failed worktree branch",
        )
        .await;
}

pub(crate) async fn remove(
    home: &Path,
    allow_local_git: bool,
    reference: &WorktreeRef,
) -> Result<()> {
    validate_remove_request(reference)?;
    safe_component(&reference.workspace, "workspace name")?;
    safe_component(&reference.name, "worktree name")?;
    let repository = home.join(&reference.workspace);
    if !repository_exists(&repository)? {
        return Ok(());
    }
    let path = repository.join(".worktrees").join(&reference.name);
    let git = GitRunner::new(allow_local_git);
    if real_directory_or_absent(&path)? {
        git.checked(
            &repository,
            [
                OsString::from("worktree"),
                OsString::from("remove"),
                OsString::from("--force"),
                path.as_os_str().to_owned(),
            ],
            "remove worktree",
        )
        .await?;
    } else {
        git.checked(
            &repository,
            ["worktree", "prune", "--expire", "now"],
            "prune removed worktree",
        )
        .await?;
    }
    let branch = branch_name(&reference.name);
    if branch_exists(&git, &repository, &branch).await? {
        git.checked(
            &repository,
            ["branch", "-D", branch.as_str()],
            "delete worktree branch",
        )
        .await?;
    }
    let parent = repository.join(".worktrees");
    if path_is_directory(&parent) {
        crate::fs_durability::sync_directory(&parent)
            .map_err(worktree_io("sync worktree removal"))?;
    }
    crate::fs_durability::sync_tree(&repository.join(".git")).map_err(worktree_io(
        "sync repository metadata after removing worktree",
    ))?;
    Ok(())
}

pub(crate) async fn preflight_remove(
    home: &Path,
    allow_local_git: bool,
    reference: &WorktreeRef,
) -> Result<()> {
    validate_remove_request(reference)?;
    safe_component(&reference.workspace, "workspace name")?;
    safe_component(&reference.name, "worktree name")?;
    let repository = home.join(&reference.workspace);
    if !repository_exists(&repository)? {
        return Ok(());
    }
    let path = expected_path(home, &reference.workspace, &reference.name);
    real_directory_or_absent(&path)?;
    let git = GitRunner::new(allow_local_git);
    branch_exists(&git, &repository, &branch_name(&reference.name)).await?;
    Ok(())
}

pub(crate) fn validate_add_request(request: &WorktreeRequest) -> Result<()> {
    let expected = format!("worktree:{}:{}:add:v1", request.workspace, request.name);
    validate_operation(
        request.has_valid_sha256(),
        &request.operation_id,
        &request.request_sha256,
        &expected,
    )
}

pub(crate) fn validate_remove_request(request: &WorktreeRef) -> Result<()> {
    let expected = format!("worktree:{}:{}:remove:v1", request.workspace, request.name);
    validate_operation(
        request.has_valid_sha256(),
        &request.operation_id,
        &request.request_sha256,
        &expected,
    )
}

fn validate_operation(
    digest_matches: bool,
    operation_id: &str,
    request_sha256: &str,
    expected: &str,
) -> Result<()> {
    let lower_sha256 = request_sha256.len() == 64
        && request_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !digest_matches || operation_id != expected || operation_id.len() > 256 || !lower_sha256 {
        return Err(RuntimeError::rejected(
            "worktree_operation_invalid",
            "worktree operation identity is invalid",
        ));
    }
    Ok(())
}

pub(crate) fn parse_path(home: &Path, candidate: &Path) -> Option<(String, String)> {
    let relative = candidate.strip_prefix(home).ok()?;
    let parts = relative.components().collect::<Vec<_>>();
    if parts.len() != 3
        || !matches!(parts[0], Component::Normal(_))
        || !matches!(parts[1], Component::Normal(part) if part == ".worktrees")
        || !matches!(parts[2], Component::Normal(_))
    {
        return None;
    }
    let workspace = parts[0].as_os_str().to_str()?.to_string();
    let worktree = parts[2].as_os_str().to_str()?.to_string();
    safe_component(&workspace, "workspace").ok()?;
    safe_component(&worktree, "worktree").ok()?;
    Some((workspace, worktree))
}

pub(crate) fn expected_path(home: &Path, workspace: &str, name: &str) -> PathBuf {
    home.join(workspace).join(".worktrees").join(name)
}

fn require_repository(repository: &Path) -> Result<()> {
    if !repository_exists(repository)? {
        return Err(RuntimeError::rejected(
            "workspace_not_found",
            "workspace is not a main Git checkout",
        ));
    }
    Ok(())
}

fn repository_exists(repository: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(repository) {
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && path_is_directory(&repository.join(".git")) =>
        {
            Ok(true)
        }
        Ok(_) => Err(RuntimeError::rejected(
            "workspace_not_found",
            "workspace is not a main Git checkout",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(worktree_io("inspect workspace repository")(error)),
    }
}

fn ensure_worktree_parent(parent: &Path) -> Result<()> {
    match std::fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(RuntimeError::rejected(
            "worktree_parent_invalid",
            "workspace .worktrees path is not a directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            crate::fs_durability::create_dir(parent).map_err(|error| {
                RuntimeError::unavailable(
                    "worktree_io_failed",
                    format!("create .worktrees directory: {error}"),
                )
            })
        }
        Err(error) => Err(RuntimeError::unavailable(
            "worktree_io_failed",
            format!("inspect .worktrees directory: {error}"),
        )),
    }
}

fn real_directory_or_absent(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(RuntimeError::rejected(
            "worktree_invalid",
            "worktree path is not a real directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(worktree_io("inspect worktree path")(error)),
    }
}

async fn symbolic_branch(git: &GitRunner, path: &Path) -> Result<String> {
    git.checked(
        path,
        ["symbolic-ref", "--quiet", "--short", "HEAD"],
        "inspect worktree branch",
    )
    .await
    .map(|output| output.stdout.trim().to_string())
    .map_err(|error| {
        if error.retryable {
            error
        } else {
            add_indeterminate(&error.detail)
        }
    })
}

fn branch_name(name: &str) -> String {
    format!("cma/{name}")
}

fn add_indeterminate(detail: &str) -> RuntimeError {
    RuntimeError::rejected("worktree_add_indeterminate", detail)
}

fn entry_exists(path: &Path, operation: &'static str) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(worktree_io(operation)(error)),
    }
}

fn real_file(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_file() && !metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(worktree_io("inspect worktree Git link")(error)),
    }
}

fn worktree_io(operation: &'static str) -> impl FnOnce(std::io::Error) -> RuntimeError + Copy {
    move |error| RuntimeError::unavailable("worktree_io_failed", format!("{operation}: {error}"))
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
