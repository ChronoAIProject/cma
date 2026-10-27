//! Bounded persistent-home file operations under current Agent access.
use crate::{
    Client, Error, Result,
    http::{Request, receive_bounded},
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENCODED_BYTES: usize = MAX_BYTES.div_ceil(3) * 4;

/// A directory entry; listing includes hidden entries without following symlinks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentFileEntry {
    /// Path relative to /home.
    pub path: String,
    /// Basename.
    pub name: String,
    /// File, directory or symlink.
    pub kind: AgentFileKind,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Optional content digest.
    pub sha256: Option<String>,
    /// Whether this entry is hidden.
    pub hidden: bool,
}
/// Entry kind; symlinks are listed but never traversed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
    /// Symbolic link.
    Symlink,
}
/// One lazily listed directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentFileList {
    /// Relative directory path; empty names /home.
    pub path: String,
    /// Bounded entries including dotfiles.
    pub entries: Vec<AgentFileEntry>,
}
/// Exact file bytes encoded as base64 with their compare-and-swap digest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentFileContent {
    /// Relative file path.
    pub path: String,
    /// Decoded byte count.
    pub size_bytes: u64,
    /// SHA-256 used for subsequent versioned saves.
    pub sha256: String,
    /// Base64 bytes, never executed or interpreted by this SDK.
    pub content_base64: String,
}
/// A create-only or compare-and-swap file write; mutations are never retried automatically.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileWrite {
    /// Relative target path.
    pub path: String,
    /// Base64 content; empty for directory creation.
    pub content_base64: String,
    /// Last-read digest. Omit only for create-only writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_sha256: Option<String>,
    /// Create a directory instead of a regular file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<bool>,
}
/// Exact mutation result; unused fields are null.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentFileMutation {
    /// Relative target path.
    pub path: String,
    /// Written size, if applicable.
    pub size_bytes: Option<u64>,
    /// Written digest, if applicable.
    pub sha256: Option<String>,
    /// Whether the target was deleted.
    pub deleted: Option<bool>,
}
/// File APIs for one configured client, not a retained permission grant.
pub struct AgentFiles<'a> {
    client: &'a Client,
}
impl Client {
    /// Browse or manipulate the persistent /home under current Agent permission.
    pub fn agent_files(&self) -> AgentFiles<'_> {
        AgentFiles { client: self }
    }
}
impl AgentFiles<'_> {
    /// List exactly one directory. Empty path names /home.
    pub async fn list(&self, agent_id: &str, path: &str) -> Result<AgentFileList> {
        self.client
            .get(
                &["agents", agent_id, "files", "tree"],
                &[("path", path.into())],
            )
            .await
    }
    /// Read content up to max_bytes (at most 32 MiB); None uses the server preview bound.
    pub async fn read(
        &self,
        agent_id: &str,
        path: &str,
        max_bytes: Option<u64>,
    ) -> Result<AgentFileContent> {
        if max_bytes.is_some_and(|n| n == 0 || n > MAX_BYTES as u64) {
            return Err(Error::validation("max_bytes must be between 1 and 32 MiB"));
        }
        let segments = ["agents", agent_id, "files", "content"];
        let mut query = vec![("path", path.into())];
        if let Some(n) = max_bytes {
            query.push(("max_bytes", n.to_string()));
        }
        let mut request = Request::get(&segments);
        request.query = &query;
        let bytes = receive_bounded(&self.client.inner, request, MAX_ENCODED_BYTES + 4096).await?;
        serde_json::from_slice(&bytes.body)
            .map_err(|_| Error::protocol("invalid file content response"))
    }
    /// Download opaque bytes with an independent 32 MiB response bound.
    pub async fn download(&self, agent_id: &str, path: &str) -> Result<Vec<u8>> {
        let segments = ["agents", agent_id, "files", "download"];
        let query = [("path", path.into())];
        let mut request = Request::get(&segments);
        request.query = &query;
        Ok(receive_bounded(&self.client.inner, request, MAX_BYTES)
            .await?
            .body)
    }
    /// Create or version-save one path. Omit expected_sha256 only for create-only writes.
    pub async fn write(&self, agent_id: &str, body: &AgentFileWrite) -> Result<AgentFileMutation> {
        if body.content_base64.len() > MAX_ENCODED_BYTES {
            return Err(Error::validation("file content exceeds 32 MiB"));
        }
        self.client
            .operation(
                Method::POST,
                &["agents", agent_id, "files"],
                Some(body),
                StatusCode::OK,
            )
            .await
    }
    /// Delete one target, with explicit recursive authorization for nonempty directories.
    pub async fn remove(
        &self,
        agent_id: &str,
        path: &str,
        recursive: bool,
    ) -> Result<AgentFileMutation> {
        let segments = ["agents", agent_id, "files"];
        let mut query = vec![("path", path.into())];
        if recursive {
            query.push(("recursive", "true".into()));
        }
        let mut request =
            Request::operation::<()>(Method::DELETE, &segments, None, StatusCode::OK)?;
        request.query = &query;
        self.client.request(request).await
    }
}
