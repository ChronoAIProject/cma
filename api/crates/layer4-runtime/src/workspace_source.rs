//! Git-backed workspace source preparation and per-conversation worktrees.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;
use url::Url;

pub const CAPABILITY_VERSION: u32 = 2;
const GIT_DEADLINE: Duration = Duration::from_secs(120);
const MAX_GIT_OUTPUT_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSourceRequest {
    pub url: String,
    pub reference: String,
    pub directory: String,
    pub workspace_epoch: u64,
}

impl WorkspaceSourceRequest {
    pub fn validate(&self) -> Result<(), WorkspaceSourceError> {
        let invalid_url = || {
            WorkspaceSourceError::Invalid(
                "url must be a credential-free HTTPS URL without a query or fragment",
            )
        };
        if self.url.len() > 512 {
            return Err(invalid_url());
        }
        let parsed = Url::parse(&self.url).map_err(|_| invalid_url())?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(invalid_url());
        }
        if self.workspace_epoch == 0 {
            return Err(WorkspaceSourceError::Invalid(
                "workspace_epoch must be positive",
            ));
        }
        validate_safe_segment(&self.directory)?;
        validate_git_selector("reference", &self.reference)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceSourceReceipt {
    pub workspace_epoch: u64,
    pub source_revision: String,
    pub baseline_manifest_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTreeRequest {
    pub name: String,
    pub base: String,
}

impl WorkspaceTreeRequest {
    pub fn validate(&self) -> Result<(), WorkspaceSourceError> {
        validate_tree_name(&self.name)?;
        validate_git_selector("base", &self.base)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceTreeReceipt {
    pub path: String,
}

#[derive(Debug)]
pub enum WorkspaceSourceError {
    Invalid(&'static str),
    Conflict(&'static str),
    NotFound,
    Git {
        operation: &'static str,
        stderr: String,
    },
    Timeout(&'static str),
    Runtime(String),
}

impl std::fmt::Display for WorkspaceSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(detail) | Self::Conflict(detail) => formatter.write_str(detail),
            Self::NotFound => formatter.write_str("workspace tree was not found"),
            Self::Git { operation, stderr } if stderr.is_empty() => {
                write!(formatter, "git {operation} failed")
            }
            Self::Git { operation, stderr } => {
                write!(formatter, "git {operation} failed: {stderr}")
            }
            Self::Timeout(operation) => write!(formatter, "git {operation} exceeded its deadline"),
            Self::Runtime(detail) => write!(formatter, "workspace source runtime failed: {detail}"),
        }
    }
}

impl std::error::Error for WorkspaceSourceError {}

#[derive(Debug, Clone)]
struct PreparedSource {
    request: WorkspaceSourceRequest,
    repository: PathBuf,
    trees: BTreeMap<String, PathBuf>,
    receipt: WorkspaceSourceReceipt,
}

#[derive(Debug, Default)]
pub struct WorkspaceSources {
    prepared: Option<PreparedSource>,
    git: GitRunner,
}

impl WorkspaceSources {
    /// Resolve the main repository prepared by clone or recovered from a
    /// complete-Git workspace restore. Linked worktrees carry a `.git` file;
    /// only the one main clone carries the directory accepted here.
    pub fn setup_directory(&self, workspace_root: &Path) -> Result<PathBuf, WorkspaceSourceError> {
        if let Some(prepared) = self.prepared.as_ref() {
            if is_main_repository(&prepared.repository)? {
                return Ok(prepared.repository.clone());
            }
            return Err(WorkspaceSourceError::Conflict(
                "the prepared workspace repository is no longer available",
            ));
        }

        let entries = match std::fs::read_dir(workspace_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(WorkspaceSourceError::Conflict(
                    "a workspace source has not been prepared or restored",
                ));
            }
            Err(error) => return Err(WorkspaceSourceError::Runtime(error.to_string())),
        };
        let mut repository = None;
        for entry in entries {
            let entry = entry.map_err(|error| WorkspaceSourceError::Runtime(error.to_string()))?;
            let path = entry.path();
            if !is_main_repository(&path)? {
                continue;
            }
            if repository.replace(path).is_some() {
                return Err(WorkspaceSourceError::Conflict(
                    "the restored workspace contains multiple repository roots",
                ));
            }
        }
        repository.ok_or(WorkspaceSourceError::Conflict(
            "a workspace source has not been prepared or restored",
        ))
    }

    pub async fn prepare(
        &mut self,
        workspace_root: &Path,
        request: WorkspaceSourceRequest,
    ) -> Result<WorkspaceSourceReceipt, WorkspaceSourceError> {
        request.validate()?;
        std::fs::create_dir_all(workspace_root)
            .map_err(|error| WorkspaceSourceError::Runtime(error.to_string()))?;
        let repository = workspace_root.join(&request.directory);

        if let Some(prepared) = self.prepared.as_ref() {
            if prepared.request != request {
                return Err(WorkspaceSourceError::Conflict(
                    "this runtime already has a different workspace source",
                ));
            }
        }

        match std::fs::symlink_metadata(&repository) {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    || !metadata.is_dir()
                    || !self
                        .git
                        .is_same_clone(workspace_root, &repository, &request)
                        .await? =>
            {
                return Err(WorkspaceSourceError::Conflict(
                    "the workspace source directory is occupied by a different source",
                ));
            }
            Ok(_) if self.prepared.is_none() => {
                let receipt = self
                    .git
                    .source_receipt(workspace_root, &repository, &request)
                    .await?;
                self.prepared = Some(PreparedSource {
                    request,
                    repository,
                    trees: BTreeMap::new(),
                    receipt: receipt.clone(),
                });
                return Ok(receipt);
            }
            Ok(_) => {
                return Ok(self
                    .prepared
                    .as_ref()
                    .expect("a checked prepared source exists")
                    .receipt
                    .clone())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(WorkspaceSourceError::Runtime(error.to_string())),
        }

        let args = vec![
            OsString::from("clone"),
            OsString::from("--depth"),
            OsString::from("1"),
            OsString::from("--no-tags"),
            OsString::from("--branch"),
            OsString::from(&request.reference),
            OsString::from("--single-branch"),
            OsString::from(&request.url),
            repository.as_os_str().to_owned(),
        ];
        let output = self.git.run(workspace_root, &args, "clone").await?;
        if !output.success {
            let _ = std::fs::remove_dir_all(&repository);
            return Err(WorkspaceSourceError::Git {
                operation: "clone",
                stderr: output.stderr,
            });
        }
        let receipt = self
            .git
            .source_receipt(workspace_root, &repository, &request)
            .await?;
        self.prepared = Some(PreparedSource {
            request,
            repository,
            trees: BTreeMap::new(),
            receipt: receipt.clone(),
        });
        Ok(receipt)
    }

    pub async fn add_tree(
        &mut self,
        workspace_root: &Path,
        request: WorkspaceTreeRequest,
    ) -> Result<WorkspaceTreeReceipt, WorkspaceSourceError> {
        request.validate()?;
        let prepared = self
            .prepared
            .as_mut()
            .ok_or(WorkspaceSourceError::Conflict(
                "a workspace source has not been prepared",
            ))?;
        let path = workspace_root.join("worktrees").join(&request.name);
        let path_occupied = match std::fs::symlink_metadata(&path) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(WorkspaceSourceError::Runtime(error.to_string())),
        };
        if prepared.trees.contains_key(&request.name) || path_occupied {
            return Err(WorkspaceSourceError::Conflict(
                "the workspace tree name is already occupied",
            ));
        }
        let parent = path
            .parent()
            .expect("a worktree path has its workspace parent");
        std::fs::create_dir_all(parent)
            .map_err(|error| WorkspaceSourceError::Runtime(error.to_string()))?;
        let parent_metadata = std::fs::symlink_metadata(parent)
            .map_err(|error| WorkspaceSourceError::Runtime(error.to_string()))?;
        if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
            return Err(WorkspaceSourceError::Conflict(
                "the workspace worktree directory is occupied",
            ));
        }
        let branch = format!("chat/{}", request.name);
        let args = vec![
            OsString::from("-C"),
            prepared.repository.as_os_str().to_owned(),
            OsString::from("worktree"),
            OsString::from("add"),
            OsString::from("-b"),
            OsString::from(branch),
            OsString::from("--"),
            path.as_os_str().to_owned(),
            OsString::from(request.base),
        ];
        let output = self.git.run(workspace_root, &args, "worktree add").await?;
        if !output.success {
            let _ = std::fs::remove_dir_all(&path);
            return Err(WorkspaceSourceError::Git {
                operation: "worktree add",
                stderr: output.stderr,
            });
        }
        prepared.trees.insert(request.name, path.clone());
        Ok(WorkspaceTreeReceipt {
            path: path.to_string_lossy().into_owned(),
        })
    }

    pub async fn remove_tree(
        &mut self,
        workspace_root: &Path,
        name: &str,
    ) -> Result<(), WorkspaceSourceError> {
        validate_tree_name(name)?;
        let prepared = self
            .prepared
            .as_mut()
            .ok_or(WorkspaceSourceError::NotFound)?;
        let path = prepared
            .trees
            .get(name)
            .cloned()
            .ok_or(WorkspaceSourceError::NotFound)?;
        let remove = vec![
            OsString::from("-C"),
            prepared.repository.as_os_str().to_owned(),
            OsString::from("worktree"),
            OsString::from("remove"),
            OsString::from("--force"),
            path.as_os_str().to_owned(),
        ];
        let output = self
            .git
            .run(workspace_root, &remove, "worktree remove")
            .await?;
        if !output.success {
            return Err(WorkspaceSourceError::Git {
                operation: "worktree remove",
                stderr: output.stderr,
            });
        }
        prepared.trees.remove(name);

        let prune = vec![
            OsString::from("-C"),
            prepared.repository.as_os_str().to_owned(),
            OsString::from("worktree"),
            OsString::from("prune"),
        ];
        let output = self
            .git
            .run(workspace_root, &prune, "worktree prune")
            .await?;
        if !output.success {
            return Err(WorkspaceSourceError::Git {
                operation: "worktree prune",
                stderr: output.stderr,
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn with_url_rewrite(from: String, to: String) -> Self {
        Self {
            prepared: None,
            git: GitRunner {
                extra_config: vec![(format!("url.{to}.insteadOf"), from)],
            },
        }
    }
}

fn is_main_repository(path: &Path) -> Result<bool, WorkspaceSourceError> {
    let repository = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(WorkspaceSourceError::Runtime(error.to_string())),
    };
    if repository.file_type().is_symlink() || !repository.is_dir() {
        return Ok(false);
    }
    let git = match std::fs::symlink_metadata(path.join(".git")) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(WorkspaceSourceError::Runtime(error.to_string())),
    };
    Ok(!git.file_type().is_symlink() && git.is_dir())
}

pub fn validate_tree_name(value: &str) -> Result<(), WorkspaceSourceError> {
    if value.is_empty()
        || value.len() > 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(WorkspaceSourceError::Invalid(
            "name must match [a-z0-9-]{1,40}",
        ));
    }
    Ok(())
}

fn validate_safe_segment(value: &str) -> Result<(), WorkspaceSourceError> {
    if value.is_empty()
        || value.len() > 255
        || matches!(value, "." | "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(WorkspaceSourceError::Invalid(
            "directory must be one safe path segment",
        ));
    }
    Ok(())
}

fn validate_git_selector(field: &'static str, value: &str) -> Result<(), WorkspaceSourceError> {
    if value.is_empty()
        || value.len() > 200
        || value.starts_with('-')
        || value.chars().any(char::is_whitespace)
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(WorkspaceSourceError::Invalid(match field {
            "reference" => {
                "reference must be non-empty, at most 200 bytes, and contain no whitespace"
            }
            _ => "base must be non-empty, at most 200 bytes, and contain no whitespace",
        }));
    }
    Ok(())
}

#[derive(Debug, Default)]
struct GitRunner {
    extra_config: Vec<(String, String)>,
}

struct GitOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

impl GitRunner {
    async fn source_receipt(
        &self,
        workspace_root: &Path,
        repository: &Path,
        request: &WorkspaceSourceRequest,
    ) -> Result<WorkspaceSourceReceipt, WorkspaceSourceError> {
        let commit = self
            .run(
                workspace_root,
                &[
                    OsString::from("-C"),
                    repository.as_os_str().to_owned(),
                    OsString::from("rev-parse"),
                    OsString::from("--verify"),
                    OsString::from("HEAD^{commit}"),
                ],
                "resolve source commit",
            )
            .await?;
        let tree = self
            .run(
                workspace_root,
                &[
                    OsString::from("-C"),
                    repository.as_os_str().to_owned(),
                    OsString::from("rev-parse"),
                    OsString::from("--verify"),
                    OsString::from("HEAD^{tree}"),
                ],
                "resolve source tree",
            )
            .await?;
        let source_revision = commit.stdout.trim();
        let tree_id = tree.stdout.trim();
        if !commit.success
            || !tree.success
            || !is_object_id(source_revision)
            || !is_object_id(tree_id)
        {
            return Err(WorkspaceSourceError::Conflict(
                "the prepared source has no exact Git commit and tree",
            ));
        }
        let mut baseline = Sha256::new();
        baseline.update(b"cma-chat-git-baseline-v1\0");
        baseline.update(source_revision.as_bytes());
        baseline.update(tree_id.as_bytes());
        Ok(WorkspaceSourceReceipt {
            workspace_epoch: request.workspace_epoch,
            source_revision: source_revision.to_ascii_lowercase(),
            baseline_manifest_digest: hex(&baseline.finalize()),
        })
    }

    async fn is_same_clone(
        &self,
        workspace_root: &Path,
        repository: &Path,
        request: &WorkspaceSourceRequest,
    ) -> Result<bool, WorkspaceSourceError> {
        let remote = vec![
            OsString::from("-C"),
            repository.as_os_str().to_owned(),
            OsString::from("config"),
            OsString::from("--get"),
            OsString::from("remote.origin.url"),
        ];
        let remote = self.run(workspace_root, &remote, "inspect clone").await?;
        if !remote.success || remote.stdout.trim() != request.url {
            return Ok(false);
        }
        let reference = vec![
            OsString::from("-C"),
            repository.as_os_str().to_owned(),
            OsString::from("symbolic-ref"),
            OsString::from("--quiet"),
            OsString::from("--short"),
            OsString::from("HEAD"),
        ];
        let reference = self
            .run(workspace_root, &reference, "inspect clone")
            .await?;
        Ok(reference.success && reference.stdout.trim() == request.reference)
    }

    async fn run(
        &self,
        workspace_root: &Path,
        args: &[OsString],
        operation: &'static str,
    ) -> Result<GitOutput, WorkspaceSourceError> {
        let mut command = tokio::process::Command::new("git");
        command
            .args(args)
            .current_dir(workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "Never")
            .env("GIT_CONFIG_COUNT", self.extra_config.len().to_string());
        for (index, (key, value)) in self.extra_config.iter().enumerate() {
            command.env(format!("GIT_CONFIG_KEY_{index}"), key);
            command.env(format!("GIT_CONFIG_VALUE_{index}"), value);
        }

        let mut child = command
            .spawn()
            .map_err(|error| WorkspaceSourceError::Runtime(error.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| WorkspaceSourceError::Runtime("git stdout was unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| WorkspaceSourceError::Runtime("git stderr was unavailable".into()))?;
        let stdout = tokio::spawn(read_bounded(stdout));
        let stderr = tokio::spawn(read_bounded(stderr));

        let status = match tokio::time::timeout(GIT_DEADLINE, child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => return Err(WorkspaceSourceError::Runtime(error.to_string())),
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(WorkspaceSourceError::Timeout(operation));
            }
        };
        let stdout = stdout
            .await
            .map_err(|error| WorkspaceSourceError::Runtime(error.to_string()))?
            .map_err(|error| WorkspaceSourceError::Runtime(error.to_string()))?;
        let stderr = stderr
            .await
            .map_err(|error| WorkspaceSourceError::Runtime(error.to_string()))?
            .map_err(|error| WorkspaceSourceError::Runtime(error.to_string()))?;
        Ok(GitOutput {
            success: status.success(),
            stdout: output_excerpt(&stdout),
            stderr: output_excerpt(&stderr),
        })
    }
}

fn is_object_id(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hex(bytes: &[u8]) -> String {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(CHARS[usize::from(byte >> 4)]));
        out.push(char::from(CHARS[usize::from(byte & 0x0f)]));
    }
    out
}

async fn read_bounded<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> std::io::Result<Vec<u8>> {
    let mut retained = Vec::with_capacity(MAX_GIT_OUTPUT_BYTES);
    let mut buffer = [0_u8; 4 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(retained);
        }
        let remaining = MAX_GIT_OUTPUT_BYTES.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

fn output_excerpt(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
#[path = "workspace_source_tests.rs"]
mod tests;
