use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::CommandReceipt;
use crate::{
    DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES, DEFAULT_COMMAND_TIMEOUT_SECONDS,
    DEFAULT_TOTAL_TIMEOUT_SECONDS,
};

/// How a workspace root is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceSource {
    RepositorySnapshot {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    PublicRepositorySnapshot {
        url: String,
        commit: String,
    },
    Clone {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    Init,
}

/// Build the credential-free archive URL for an exact public GitHub commit.
/// Only canonical repository locators and lowercase commit IDs are accepted.
pub fn public_repository_archive_url(repository: &str, commit: &str) -> Option<url::Url> {
    let path = repository.strip_prefix("https://github.com/")?;
    let (owner, name) = path.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && !matches!(part, "." | "..")
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    if !valid(owner)
        || !valid(name)
        || commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let mut url = url::Url::parse("https://codeload.github.com").ok()?;
    url.path_segments_mut()
        .ok()?
        .extend([owner, name, "zip", commit]);
    Some(url)
}

/// Prepare one named directory directly below the sandbox home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRequest {
    pub operation_id: String,
    pub request_sha256: String,
    pub workspace_id: String,
    pub name: String,
    pub source: WorkspaceSource,
    pub clone_depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_connection_id: Option<String>,
}

impl WorkspaceRequest {
    pub fn new(
        operation_id: impl Into<String>,
        workspace_id: impl Into<String>,
        name: impl Into<String>,
        source: WorkspaceSource,
    ) -> Self {
        Self::with_clone_depth(operation_id, workspace_id, name, source, 50)
    }

    pub fn with_clone_depth(
        operation_id: impl Into<String>,
        workspace_id: impl Into<String>,
        name: impl Into<String>,
        source: WorkspaceSource,
        clone_depth: u32,
    ) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            request_sha256: String::new(),
            workspace_id: workspace_id.into(),
            name: name.into(),
            source,
            clone_depth,
            repository_connection_id: None,
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    pub fn with_repository_connection(mut self, connection_id: Option<String>) -> Self {
        self.repository_connection_id = connection_id;
        self.request_sha256 = self.computed_sha256();
        self
    }

    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.workspace.prepare.v2\0");
        digest.update(self.clone_depth.to_be_bytes());
        field(&mut digest, &self.workspace_id);
        field(&mut digest, &self.name);
        match &self.source {
            WorkspaceSource::RepositorySnapshot { url, reference } => {
                digest.update([3]);
                field(&mut digest, url);
                optional_field(&mut digest, reference.as_deref());
            }
            WorkspaceSource::PublicRepositorySnapshot { url, commit } => {
                digest.update([4]);
                field(&mut digest, url);
                field(&mut digest, commit);
            }
            WorkspaceSource::Clone { url, reference } => {
                digest.update([1]);
                field(&mut digest, url);
                optional_field(&mut digest, reference.as_deref());
            }
            WorkspaceSource::Init => digest.update([2]),
        }
        if let Some(connection) = &self.repository_connection_id {
            field(&mut digest, connection);
        }
        hex(&digest.finalize())
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

/// The canonical workspace facts resolved by the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceReceipt {
    pub operation_id: String,
    pub request_sha256: String,
    pub workspace_id: String,
    pub name: String,
    pub path: String,
    pub resolved_commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_commit: Option<String>,
}

/// Run workspace-profile setup commands in its main checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSetupRequest {
    pub operation_id: String,
    pub request_sha256: String,
    pub workspace_id: String,
    pub name: String,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(rename = "timeout_s")]
    pub timeout_seconds: u64,
    #[serde(rename = "total_timeout_s")]
    pub total_timeout_seconds: u64,
    #[serde(rename = "output_limit")]
    pub output_limit_bytes: u32,
}

impl WorkspaceSetupRequest {
    pub fn new(
        operation_id: impl Into<String>,
        workspace_id: impl Into<String>,
        name: impl Into<String>,
        commands: Vec<String>,
    ) -> Self {
        Self::bounded(
            operation_id,
            workspace_id,
            name,
            commands,
            DEFAULT_COMMAND_TIMEOUT_SECONDS,
            DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        )
    }

    pub fn bounded(
        operation_id: impl Into<String>,
        workspace_id: impl Into<String>,
        name: impl Into<String>,
        commands: Vec<String>,
        timeout_seconds: u64,
        output_limit_bytes: u32,
    ) -> Self {
        Self::with_timeouts(
            operation_id,
            workspace_id,
            name,
            commands,
            timeout_seconds,
            DEFAULT_TOTAL_TIMEOUT_SECONDS,
            output_limit_bytes,
        )
    }

    pub fn with_timeouts(
        operation_id: impl Into<String>,
        workspace_id: impl Into<String>,
        name: impl Into<String>,
        commands: Vec<String>,
        timeout_seconds: u64,
        total_timeout_seconds: u64,
        output_limit_bytes: u32,
    ) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            request_sha256: String::new(),
            workspace_id: workspace_id.into(),
            name: name.into(),
            commands,
            timeout_seconds,
            total_timeout_seconds,
            output_limit_bytes,
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.workspace.setup.v2\0");
        field(&mut digest, &self.workspace_id);
        field(&mut digest, &self.name);
        digest.update(self.timeout_seconds.to_be_bytes());
        digest.update(self.total_timeout_seconds.to_be_bytes());
        digest.update(self.output_limit_bytes.to_be_bytes());
        digest.update(
            u32::try_from(self.commands.len())
                .unwrap_or(u32::MAX)
                .to_be_bytes(),
        );
        for command in &self.commands {
            field(&mut digest, command);
        }
        hex(&digest.finalize())
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

/// Proof that every setup command completed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupReceipt {
    pub operation_id: String,
    pub request_sha256: String,
    pub workspace_id: String,
    #[serde(default)]
    pub commands: Vec<CommandReceipt>,
}

/// Remove one workspace using an aggregate-specific replay identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRemoveRequest {
    pub operation_id: String,
    pub request_sha256: String,
    pub workspace_id: String,
    pub name: String,
}

impl WorkspaceRemoveRequest {
    pub fn new(
        operation_id: impl Into<String>,
        workspace_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            request_sha256: String::new(),
            workspace_id: workspace_id.into(),
            name: name.into(),
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.workspace.remove.v1\0");
        field(&mut digest, &self.workspace_id);
        field(&mut digest, &self.name);
        hex(&digest.finalize())
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

/// Correlated proof that workspace removal completed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRemoveReceipt {
    pub operation_id: String,
    pub request_sha256: String,
    pub workspace_id: String,
}

fn field(digest: &mut Sha256, value: &str) {
    digest.update(u32::try_from(value.len()).unwrap_or(u32::MAX).to_be_bytes());
    digest.update(value.as_bytes());
}

fn optional_field(digest: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            digest.update([1]);
            field(digest, value);
        }
        None => digest.update([0]),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Add one independently writable Git worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeRequest {
    pub operation_id: String,
    pub request_sha256: String,
    pub workspace: String,
    pub name: String,
    pub base: String,
}

impl WorktreeRequest {
    pub fn new(
        operation_id: impl Into<String>,
        workspace: impl Into<String>,
        name: impl Into<String>,
        base: impl Into<String>,
    ) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            request_sha256: String::new(),
            workspace: workspace.into(),
            name: name.into(),
            base: base.into(),
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.worktree.add.v1\0");
        field(&mut digest, &self.workspace);
        field(&mut digest, &self.name);
        field(&mut digest, &self.base);
        hex(&digest.finalize())
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

/// Stable reference used to remove a worktree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeRef {
    pub operation_id: String,
    pub request_sha256: String,
    pub workspace: String,
    pub name: String,
}

impl WorktreeRef {
    pub fn new(
        operation_id: impl Into<String>,
        workspace: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            request_sha256: String::new(),
            workspace: workspace.into(),
            name: name.into(),
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.worktree.remove.v1\0");
        field(&mut digest, &self.workspace);
        field(&mut digest, &self.name);
        hex(&digest.finalize())
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

/// Paths and branch resolved by the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeReceipt {
    pub operation_id: String,
    pub request_sha256: String,
    pub workspace: String,
    pub name: String,
    pub path: String,
    pub branch: String,
    pub base_commit: String,
}

/// Correlated proof that one worktree and its private branch were removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeRemoveReceipt {
    pub operation_id: String,
    pub request_sha256: String,
    pub workspace: String,
    pub name: String,
}

#[cfg(test)]
#[path = "workspace_digest_tests.rs"]
mod digest_tests;
