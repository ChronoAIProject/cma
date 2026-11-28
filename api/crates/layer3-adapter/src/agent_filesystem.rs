//! Safe file-management requests for the retained runtime home.
//!
//! Paths are always relative to the runtime's mounted home.  The runtime
//! validates and confines them before touching the filesystem; this module is
//! only the typed contract between layer 2/3 and the supervisor.
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{AdapterError, Result};

pub const MAX_AGENT_FILE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_AGENT_FILE_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileEntry {
    /// Normalized path relative to the mounted home; the root is the empty string.
    pub path: String,
    pub name: String,
    pub kind: AgentFileKind,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileListRequest {
    /// Determined by resource authorization; never accepted from a public request.
    pub owner_access: bool,
    /// Provider credential/control paths, relative to /home, supplied by the adapter.
    pub protected_paths: Vec<String>,
    pub sandbox_id: String,
    pub runtime_generation: u64,
    pub boot_id: String,
    /// Empty path addresses the mounted home root.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileListReceipt {
    pub path: String,
    pub entries: Vec<AgentFileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileReadRequest {
    /// Determined by resource authorization; never accepted from a public request.
    pub owner_access: bool,
    /// Provider credential/control paths, relative to /home, supplied by the adapter.
    pub protected_paths: Vec<String>,
    pub sandbox_id: String,
    pub runtime_generation: u64,
    pub boot_id: String,
    /// When supplied, the Runtime proves this mounted home is ready in the read itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_identity: Option<String>,
    pub path: String,
    pub max_bytes: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileReadReceipt {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub content_base64: String,
}

impl AgentFileReadReceipt {
    pub fn content(&self) -> Result<Vec<u8>> {
        if self.size_bytes > MAX_AGENT_FILE_BYTES
            || self.content_base64.len() as u64 > MAX_AGENT_FILE_BYTES.div_ceil(3) * 4
        {
            return Err(AdapterError::Rejected(
                "file response exceeds its size limit".into(),
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.content_base64)
            .map_err(|_| AdapterError::Rejected("runtime returned invalid file encoding".into()))?;
        if bytes.len() as u64 != self.size_bytes
            || format!("{:x}", Sha256::digest(&bytes)) != self.sha256
        {
            return Err(AdapterError::Rejected(
                "runtime returned an invalid file digest".into(),
            ));
        }
        Ok(bytes)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileWriteRequest {
    /// Determined by resource authorization; never accepted from a public request.
    pub owner_access: bool,
    /// Provider credential/control paths, relative to /home, supplied by the adapter.
    pub protected_paths: Vec<String>,
    pub sandbox_id: String,
    pub runtime_generation: u64,
    pub boot_id: String,
    pub path: String,
    /// If supplied, the existing file must have this digest.  This protects
    /// the editor from overwriting a concurrent save.
    pub expected_sha256: Option<String>,
    pub content_base64: String,
    #[serde(default)]
    pub directory: bool,
}

impl AgentFileWriteRequest {
    pub fn content(&self) -> Result<Vec<u8>> {
        if self.content_base64.len() as u64 > MAX_AGENT_FILE_BYTES.div_ceil(3) * 4 {
            return Err(AdapterError::Rejected("file exceeds the size limit".into()));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.content_base64)
            .map_err(|_| AdapterError::Rejected("file content encoding is invalid".into()))?;
        if bytes.len() as u64 > MAX_AGENT_FILE_BYTES {
            return Err(AdapterError::Rejected("file exceeds the size limit".into()));
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileWriteReceipt {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileDeleteRequest {
    /// Determined by resource authorization; never accepted from a public request.
    pub owner_access: bool,
    /// Provider credential/control paths, relative to /home, supplied by the adapter.
    pub protected_paths: Vec<String>,
    pub sandbox_id: String,
    pub runtime_generation: u64,
    pub boot_id: String,
    pub path: String,
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFileDeleteReceipt {
    pub path: String,
    pub deleted: bool,
}

pub fn valid_identity(sandbox_id: &str, generation: u64, boot_id: &str) -> bool {
    !sandbox_id.is_empty()
        && sandbox_id.len() <= 256
        && !sandbox_id.chars().any(char::is_control)
        && generation != 0
        && boot_id.len() == 32
        && boot_id.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn valid_path(path: &str) -> bool {
    if path.len() > 1_024 || path.contains('\0') || path.contains('\\') {
        return false;
    }
    if path.is_empty() {
        return true;
    }
    let p = std::path::Path::new(path);
    !p.is_absolute()
        && p.components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
        && p.to_string_lossy() == path
}

impl std::fmt::Debug for AgentFileReadReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFileReadReceipt")
            .field("path", &self.path)
            .field("size_bytes", &self.size_bytes)
            .field("sha256", &self.sha256)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for AgentFileWriteRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFileWriteRequest")
            .field("path", &self.path)
            .field("runtime_generation", &self.runtime_generation)
            .field("directory", &self.directory)
            .field("encoded_bytes", &self.content_base64.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_contract_accepts_hidden_paths_and_rejects_escape_selectors() {
        for path in [
            "",
            ".codex/config.toml",
            "cma/project/文件.txt",
            "attachments/photo.png",
        ] {
            assert!(valid_path(path));
        }
        for path in [
            "/etc/passwd",
            "../outside",
            "cma/../../outside",
            "cma\\outside",
            "bad\0path",
            "./cma",
        ] {
            assert!(!valid_path(path), "{path}");
        }
    }
    #[test]
    fn file_content_is_bounded_verified_and_never_formatted_in_debug_output() {
        let bytes = b"private contents";
        let receipt = AgentFileReadReceipt {
            path: "secret".into(),
            size_bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        };
        assert_eq!(receipt.content().unwrap(), bytes);
        assert!(!format!("{receipt:?}").contains(&receipt.content_base64));
        let mut corrupt = receipt.clone();
        corrupt.size_bytes += 1;
        assert!(corrupt.content().is_err());
        corrupt.size_bytes = MAX_AGENT_FILE_BYTES + 1;
        assert!(corrupt.content().is_err());
    }
}
