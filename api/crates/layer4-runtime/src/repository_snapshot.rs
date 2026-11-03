use std::io::{Cursor, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use layer3_adapter::{WorkspaceReceipt, WorkspaceRequest, WorkspaceSource};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::{fs_durability, git::GitRunner, paths, workspace_source};

const MAX_ARCHIVE: usize = 64 * 1024 * 1024;
const MAX_EXPANDED: u64 = 256 * 1024 * 1024;

pub(crate) async fn prepare(
    config: &RuntimeConfig,
    home: &Path,
    request: &WorkspaceRequest,
) -> Result<WorkspaceReceipt> {
    let WorkspaceSource::RepositorySnapshot { url, reference } = &request.source else {
        return Err(invalid("repository source is missing"));
    };
    workspace_source::validate_clone_url(url, false)?;
    paths::safe_component(&request.workspace_id, "workspace id")?;
    paths::safe_component(&request.name, "workspace name")?;
    if let Some(reference) = reference {
        paths::validate_selector(reference, "repository ref")?;
    }
    let proxy = config.github_proxy.as_ref().ok_or_else(|| {
        RuntimeError::rejected(
            "repository_connection_required",
            "GitHub connection is required before repository preparation",
        )
    })?;
    let client = reqwest::Client::builder()
        .unix_socket(proxy.socket())
        .no_proxy()
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| unavailable())?;
    let parsed = url::Url::parse(url).map_err(|_| invalid("invalid repository URL"))?;
    let repository = parsed.path().trim_matches('/').trim_end_matches(".git");
    let endpoint = format!("http://api.github.com/repos/{repository}");
    let bytes = fetch(client.get(&endpoint), "repository_unavailable").await?;
    let metadata: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| invalid("invalid repository metadata"))?;
    let reference = match reference {
        Some(reference) => reference.clone(),
        None => metadata["default_branch"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid("repository has no default branch"))?
            .to_owned(),
    };
    let commits = fetch(
        client
            .get(format!("{endpoint}/commits"))
            .query(&[("sha", reference.as_str()), ("per_page", "1")]),
        "repository_ref_not_found",
    )
    .await?;
    let commits: serde_json::Value = serde_json::from_slice(&commits)
        .map_err(|_| invalid("invalid repository commit response"))?;
    let source_commit = commits[0]["sha"]
        .as_str()
        .filter(|value| valid_commit(value))
        .ok_or_else(|| invalid("repository ref did not resolve to a commit"))?
        .to_owned();
    let path = home.join(&request.name);
    std::fs::create_dir(&path).map_err(|_| {
        RuntimeError::rejected("workspace_exists", "repository destination already exists")
    })?;
    fs_durability::write(&path.join(".cma-source-pin"), source_commit.as_bytes())
        .map_err(io_error)?;
    let bytes = fetch(
        client.get(format!("{endpoint}/zipball/{source_commit}")),
        "repository_archive_unavailable",
    )
    .await?;
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

async fn fetch(request: reqwest::RequestBuilder, missing: &'static str) -> Result<Vec<u8>> {
    let response = request
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| unavailable())?;
    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err(RuntimeError::rejected(
            "repository_connection_required",
            "GitHub authorization is missing or does not allow this repository",
        ));
    }
    if status == 404 || status == 409 || status == 422 {
        return Err(RuntimeError::rejected(
            missing,
            match missing {
                "repository_ref_not_found" => "The requested repository ref could not be found",
                _ => "The repository is unavailable or inaccessible to the connected account",
            },
        ));
    }
    if !response.status().is_success() {
        return Err(unavailable());
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
    if !matches!(source, WorkspaceSource::RepositorySnapshot { .. }) {
        return Ok(None);
    }
    let value =
        std::fs::read_to_string(path.join(".git/info/cma-source-commit")).map_err(io_error)?;
    if !valid_commit(&value) {
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
