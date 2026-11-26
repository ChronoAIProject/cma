use std::io::{Cursor, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use layer3_adapter::{WorkspaceReceipt, WorkspaceRequest, WorkspaceSource};
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::{fs_durability, git::GitRunner, paths, workspace_source};

mod error_diagnostics;
use error_diagnostics::{read_hint, ResponseHint};
mod credentialed;

const MAX_ARCHIVE: usize = 64 * 1024 * 1024;
const MAX_EXPANDED: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy)]
enum ReadStage {
    ProxySetup,
    Metadata,
    Commits,
    Archive,
}

enum ReadFailure {
    ProxyMissing,
    HttpRefused,
    HttpUnavailable,
    Timeout,
    Transport,
}

fn observe_failure(
    operation_ref: &str,
    stage: ReadStage,
    http_status: Option<u16>,
    failure: ReadFailure,
    response_hint: ResponseHint,
) {
    let proxy_present = !matches!(failure, ReadFailure::ProxyMissing);
    let stage = match stage {
        ReadStage::ProxySetup => "proxy_setup",
        ReadStage::Metadata => "metadata",
        ReadStage::Commits => "commits",
        ReadStage::Archive => "archive",
    };
    let error_class = match failure {
        ReadFailure::ProxyMissing => "proxy_missing",
        ReadFailure::HttpRefused => "http_refused",
        ReadFailure::HttpUnavailable => "http_unavailable",
        ReadFailure::Timeout => "timeout",
        ReadFailure::Transport => "transport",
    };
    tracing::warn!(
        stage,
        http_status,
        proxy_present,
        error_class,
        operation_ref,
        response_hint = response_hint.as_str(),
        "Repository preparation read failed"
    );
}

fn transport_failure(
    operation_ref: &str,
    stage: ReadStage,
    status: Option<u16>,
    error: &reqwest::Error,
) -> RuntimeError {
    let failure = if error.is_timeout() {
        ReadFailure::Timeout
    } else {
        ReadFailure::Transport
    };
    observe_failure(operation_ref, stage, status, failure, ResponseHint::Unknown);
    unavailable()
}

pub(crate) async fn prepare(
    config: &RuntimeConfig,
    home: &Path,
    request: &WorkspaceRequest,
) -> Result<WorkspaceReceipt> {
    prepare_source(config, home, request, None).await
}

pub(crate) async fn prepare_pinned(
    config: &RuntimeConfig,
    home: &Path,
    request: &WorkspaceRequest,
    source_commit: &str,
) -> Result<WorkspaceReceipt> {
    if !valid_commit(source_commit) {
        return Err(invalid("original source commit is invalid"));
    }
    prepare_source(config, home, request, Some(source_commit)).await
}

async fn prepare_source(
    config: &RuntimeConfig,
    home: &Path,
    request: &WorkspaceRequest,
    pinned_commit: Option<&str>,
) -> Result<WorkspaceReceipt> {
    paths::safe_component(&request.workspace_id, "workspace id")?;
    paths::safe_component(&request.name, "workspace name")?;
    let mut reference_digest = Sha256::new();
    reference_digest.update(b"cma.repository-preparation.operation.v1\0");
    reference_digest.update(request.operation_id.as_bytes());
    let operation_ref = format!("{:x}", reference_digest.finalize());
    let (source_commit, archive) = match &request.source {
        WorkspaceSource::RepositorySnapshot { url, reference } => {
            workspace_source::validate_clone_url(url, false)?;
            if let Some(reference) = reference {
                paths::validate_selector(reference, "repository ref")?;
            }
            credentialed::archive(
                config,
                url,
                reference,
                request.repository_connection_id.as_deref(),
                pinned_commit,
                &operation_ref,
            )
            .await?
        }
        WorkspaceSource::PublicRepositorySnapshot { url, commit } => {
            if request.repository_connection_id.is_some() {
                return Err(invalid("public source cannot carry a selected connection"));
            }
            if pinned_commit.is_some_and(|retained| retained != commit) {
                return Err(invalid(
                    "original source commit differs from the public snapshot",
                ));
            }
            let archive = layer3_adapter::public_repository_archive_url(url, commit)
                .ok_or_else(|| invalid("invalid public repository snapshot"))?;
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(120))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("CMA-public-source/1.0")
                .build()
                .map_err(|_| unavailable())?;
            (commit.clone(), client.get(archive))
        }
        _ => return Err(invalid("repository source is missing")),
    };
    let path = home.join(&request.name);
    std::fs::create_dir(&path).map_err(|_| {
        RuntimeError::rejected("workspace_exists", "repository destination already exists")
    })?;
    fs_durability::write(&path.join(".cma-source-pin"), source_commit.as_bytes())
        .map_err(io_error)?;
    let bytes = if matches!(
        request.source,
        WorkspaceSource::PublicRepositorySnapshot { .. }
    ) {
        fetch_public_archive(archive).await?
    } else {
        fetch(
            archive,
            "repository_archive_unavailable",
            &operation_ref,
            ReadStage::Archive,
        )
        .await?
    };
    let extraction_path = path.clone();
    tokio::task::spawn_blocking(move || extract(&bytes, &extraction_path))
        .await
        .map_err(|_| invalid("repository extraction stopped"))??;
    let git = GitRunner::new(false);
    git.checked(
        &path,
        ["init", "-b", "main", "."],
        "initialize source checkout",
    )
    .await?;
    let info = path.join(".git/info");
    fs_durability::create_dir_all(&info).map_err(io_error)?;
    fs_durability::rename(
        &path.join(".cma-source-pin"),
        &info.join("cma-source-commit"),
    )
    .map_err(io_error)?;
    git.checked(&path, ["add", "--all", "--", "."], "stage source checkout")
        .await?;
    git.checked(
        &path,
        [
            "-c",
            "user.name=CMA",
            "-c",
            "user.email=noreply@chrono-ai.fun",
            "commit",
            "--allow-empty",
            "-m",
            &format!("Source snapshot {source_commit}"),
        ],
        "commit source checkout",
    )
    .await?;
    workspace_source::install_worktree_metadata(&path, &request.workspace_id)?;
    let resolved_commit = workspace_source::resolve_commit(&path, &git, "HEAD").await?;
    fs_durability::sync_tree(&path).map_err(io_error)?;
    Ok(WorkspaceReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace_id: request.workspace_id.clone(),
        name: request.name.clone(),
        path: path.to_string_lossy().into_owned(),
        resolved_commit,
        source_commit: Some(source_commit),
    })
}

async fn fetch_public_archive(request: reqwest::RequestBuilder) -> Result<Vec<u8>> {
    let response = request.send().await.map_err(|_| unavailable())?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 | 404 | 410 => {
            return Err(RuntimeError::rejected(
                "repository_archive_unavailable",
                "The verified public repository archive is no longer accessible",
            ))
        }
        _ => return Err(unavailable()),
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| unavailable())?;
        if bytes.len().saturating_add(chunk.len()) > MAX_ARCHIVE {
            return Err(invalid("repository response exceeds 64 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn fetch(
    request: reqwest::RequestBuilder,
    missing: &'static str,
    operation_ref: &str,
    stage: ReadStage,
) -> Result<Vec<u8>> {
    let response = request
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| transport_failure(operation_ref, stage, None, &error))?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let (failure, error) = match status {
            401 | 403 => (
                ReadFailure::HttpRefused,
                RuntimeError::rejected(
                    "repository_connection_required",
                    "GitHub authorization is missing or does not allow this repository",
                ),
            ),
            404 | 409 | 422 => (
                ReadFailure::HttpRefused,
                RuntimeError::rejected(
                    missing,
                    match missing {
                        "repository_ref_not_found" => {
                            "The requested repository ref could not be found"
                        }
                        _ => {
                            "The repository is unavailable or inaccessible to the connected account"
                        }
                    },
                ),
            ),
            _ => (ReadFailure::HttpUnavailable, unavailable()),
        };
        let hint = read_hint(response).await;
        observe_failure(operation_ref, stage, Some(status), failure, hint);
        return Err(error);
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| transport_failure(operation_ref, stage, Some(status), &error))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_ARCHIVE {
            return Err(invalid("repository response exceeds 64 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn extract(bytes: &[u8], destination: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| invalid("invalid repository archive"))?;
    if archive.len() > 20_000 {
        return Err(invalid("repository archive has too many entries"));
    }
    let mut total = 0_u64;
    let mut root = None;
    let mut entries = Vec::new();
    let mut paths_seen = std::collections::BTreeSet::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| invalid("invalid repository archive entry"))?;
        let raw_name = entry
            .name()
            .map_err(|_| invalid("invalid archive filename"))?;
        let name = raw_name.trim_end_matches('/');
        paths::safe_relative(name, "repository archive entry")?;
        let (prefix, relative) = name.split_once('/').unwrap_or((name, ""));
        if root.as_deref().is_some_and(|root| root != prefix) {
            return Err(invalid("archive has multiple roots"));
        }
        root = Some(prefix.to_owned());
        if relative.is_empty() {
            if !entry.is_dir() {
                return Err(invalid("archive root is not a directory"));
            }
            continue;
        }
        let relative_path = paths::safe_relative(relative, "repository source path")?;
        if relative_path
            .components()
            .any(|part| part.as_os_str().eq_ignore_ascii_case(".git"))
            || relative_path.starts_with(".worktrees")
            || relative == ".cma-source-pin"
        {
            return Err(invalid("repository archive contains protected metadata"));
        }
        if relative == ".gitmodules" {
            return Err(invalid(
                "repository submodules require explicit source preparation",
            ));
        }
        if !paths_seen.insert(relative_path.clone()) {
            return Err(invalid("repository archive contains duplicate paths"));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| invalid("repository size overflow"))?;
        if total > MAX_EXPANDED {
            return Err(invalid("repository source exceeds 256 MiB"));
        }
        let mode = entry
            .unix_mode()
            .unwrap_or(if entry.is_dir() { 0o040755 } else { 0o100644 });
        let kind = mode & 0o170000;
        if !matches!(kind, 0 | 0o040000 | 0o100000 | 0o120000) {
            return Err(invalid("repository archive contains special files"));
        }
        entries.push((index, relative_path, mode, entry.is_dir()));
    }
    let files: std::collections::BTreeSet<_> = entries
        .iter()
        .filter(|(_, _, _, directory)| !directory)
        .map(|(_, path, _, _)| path)
        .collect();
    for (_, path, mode, directory) in &entries {
        if path
            .ancestors()
            .skip(1)
            .any(|ancestor| files.contains(&ancestor.to_path_buf()))
        {
            return Err(invalid("repository file or link shadows another entry"));
        }
        if mode & 0o170000 == 0o120000 && *directory {
            return Err(invalid("invalid directory link"));
        }
    }
    for (index, relative, mode, directory) in entries {
        let path = destination.join(relative);
        if directory {
            fs_durability::create_dir_all(&path).map_err(io_error)?;
            continue;
        }
        let mut entry = archive
            .by_index(index)
            .map_err(|_| invalid("invalid repository archive entry"))?;
        let mut content = Vec::new();
        let expected = entry.size();
        entry
            .by_ref()
            .take(expected.saturating_add(1))
            .read_to_end(&mut content)
            .map_err(io_error)?;
        if content.len() as u64 != expected {
            return Err(invalid("repository entry size mismatch"));
        }
        fs_durability::create_dir_all(
            path.parent()
                .ok_or_else(|| invalid("missing repository parent"))?,
        )
        .map_err(io_error)?;
        if mode & 0o170000 == 0o120000 {
            let target =
                std::str::from_utf8(&content).map_err(|_| invalid("invalid source symlink"))?;
            let target = Path::new(target);
            if target.is_absolute()
                || paths::lexical_target(destination, &path, target).is_none()
                || target
                    .components()
                    .any(|part| part.as_os_str().eq_ignore_ascii_case(".git"))
            {
                return Err(invalid("repository symlink escapes its source"));
            }
            std::os::unix::fs::symlink(target, &path).map_err(io_error)?;
        } else {
            if content.starts_with(b"version https://git-lfs.github.com/spec/v1\n") {
                return Err(invalid(
                    "repository LFS objects require explicit source preparation",
                ));
            }
            fs_durability::write(&path, &content).map_err(io_error)?;
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(if mode & 0o111 != 0 { 0o755 } else { 0o644 }),
            )
            .map_err(io_error)?;
        }
    }
    Ok(())
}

pub(crate) fn source_commit(path: &Path, source: &WorkspaceSource) -> Result<Option<String>> {
    if !matches!(
        source,
        WorkspaceSource::RepositorySnapshot { .. }
            | WorkspaceSource::PublicRepositorySnapshot { .. }
    ) {
        return Ok(None);
    }
    for directory in [path.join(".git"), path.join(".git/info")] {
        if !std::fs::symlink_metadata(directory)
            .map_err(io_error)?
            .is_dir()
        {
            return Err(invalid("source commit directory is unsafe"));
        }
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path.join(".git/info/cma-source-commit"))
        .map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() || metadata.len() > 64 {
        return Err(invalid("source commit evidence is unsafe or oversized"));
    }
    let mut value = String::new();
    file.take(65).read_to_string(&mut value).map_err(io_error)?;
    if !valid_commit(&value)
        || matches!(source, WorkspaceSource::PublicRepositorySnapshot { commit, .. } if commit != &value)
    {
        return Err(invalid("source commit evidence is invalid"));
    }
    Ok(Some(value))
}

fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn invalid(detail: &'static str) -> RuntimeError {
    RuntimeError::rejected("repository_source_invalid", detail)
}
fn unavailable() -> RuntimeError {
    RuntimeError::unavailable(
        "repository_source_unavailable",
        "GitHub source preparation is temporarily unavailable",
    )
}
fn io_error(_: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable(
        "repository_source_io",
        "Repository source could not be safely prepared",
    )
}

#[cfg(test)]
#[path = "repository_snapshot_tests.rs"]
mod tests;
