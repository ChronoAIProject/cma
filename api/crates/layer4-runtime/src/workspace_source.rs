use std::ffi::OsString;
use std::path::Path;

#[cfg(test)]
use std::path::PathBuf;

use layer3_adapter::{WorkspaceReceipt, WorkspaceRequest, WorkspaceSource};

use crate::failure::{Result, RuntimeError};
use crate::git::GitRunner;
use crate::paths::{path_is_directory, safe_component, validate_selector};

const CLONE_DEPTH: &str = "50";

pub(crate) async fn prepare(
    home: &Path,
    allow_local_git: bool,
    request: &WorkspaceRequest,
) -> Result<WorkspaceReceipt> {
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
    let result = async {
        match &request.source {
            WorkspaceSource::Clone { url, reference } => {
                clone_workspace(
                    home,
                    &path,
                    url,
                    reference.as_deref(),
                    &git,
                    allow_local_git,
                )
                .await?;
            }
            WorkspaceSource::Init => init_workspace(&path, &git).await?,
        }
        install_worktree_exclude(&path)?;
        resolve_commit(&path, &git, "HEAD").await
    }
    .await;
    let resolved_commit = match result {
        Ok(commit) => commit,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&path);
            return Err(error);
        }
    };
    Ok(WorkspaceReceipt {
        name: request.name.clone(),
        path: path.to_string_lossy().into_owned(),
        resolved_commit,
    })
}

pub(crate) fn remove(home: &Path, name: &str) -> Result<()> {
    safe_component(name, "workspace name")?;
    let path = home.join(name);
    if !path_is_directory(&path) {
        return Err(RuntimeError::rejected(
            "workspace_not_found",
            "workspace directory was not found",
        ));
    }
    if !path_is_directory(&path.join(".git")) {
        return Err(RuntimeError::rejected(
            "workspace_invalid",
            "workspace is not a main Git checkout",
        ));
    }
    let worktrees = path.join(".worktrees");
    if worktrees.exists()
        && std::fs::read_dir(&worktrees)
            .map_err(|error| io_error("inspect workspace worktrees", error))?
            .next()
            .is_some()
    {
        return Err(RuntimeError::rejected(
            "workspace_has_worktrees",
            "workspace still contains worktrees",
        ));
    }
    std::fs::remove_dir_all(path).map_err(|error| io_error("remove workspace", error))
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
        OsString::from(CLONE_DEPTH),
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
    git.checked(home, args, "clone workspace").await?;
    Ok(())
}

async fn init_workspace(path: &Path, git: &GitRunner) -> Result<()> {
    std::fs::create_dir(path).map_err(|error| io_error("create workspace", error))?;
    git.checked(path, ["init", "-b", "main", "."], "init workspace")
        .await?;
    std::fs::write(path.join(".gitkeep"), []).map_err(|error| io_error("write .gitkeep", error))?;
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

fn validate_clone_url(value: &str, allow_local: bool) -> Result<()> {
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
        || !url.username().is_empty()
        || url.password().is_some()
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

fn install_worktree_exclude(repository: &Path) -> Result<()> {
    let info = repository.join(".git/info");
    std::fs::create_dir_all(&info).map_err(|error| io_error("create Git info directory", error))?;
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
        std::fs::write(exclude, contents).map_err(|error| io_error("write Git excludes", error))?;
    }
    Ok(())
}

fn ensure_home(home: &Path) -> Result<()> {
    std::fs::create_dir_all(home).map_err(|error| io_error("create sandbox home", error))?;
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
