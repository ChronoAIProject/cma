use super::{valid_digest, DocumentError};
use std::time::Duration;

pub const MAX_BLOB_PAGE_KEYS: usize = 256;
pub const MAX_BLOB_CURSOR_BYTES: usize = 4096;

/// A maintenance caller can select one owner or one document, never a wider tree.
pub fn valid_document_blob_prefix(prefix: &str) -> bool {
    let parts: Vec<_> = prefix.split('/').collect();
    match parts.as_slice() {
        ["documents", owner, ""] => valid_digest(owner),
        ["documents", owner, document, ""] => {
            valid_digest(owner) && super::valid_id(document, "doc_")
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobDeletion {
    Deleted,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobPage {
    pub keys: Vec<String>,
    pub next: Option<String>,
}

/// Failure information belongs to this call, including concurrent maintenance calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{error}")]
pub struct BlobMaintenanceError {
    pub error: DocumentError,
    /// Suggested minimum delay before a worker tries again.
    pub retry_after: Option<Duration>,
}

impl From<DocumentError> for BlobMaintenanceError {
    fn from(error: DocumentError) -> Self {
        Self {
            error,
            retry_after: None,
        }
    }
}

#[async_trait::async_trait]
pub trait DocumentBlobMaintenance: Send + Sync {
    /// Idempotent; an absent key is success. Never follows caller URLs.
    async fn delete(&self, key: &str) -> Result<BlobDeletion, BlobMaintenanceError>;
    /// Prefix must be documents/{owner_hash}/ or documents/{owner_hash}/{doc}/.
    async fn list(
        &self,
        prefix: &str,
        cursor: Option<&str>,
    ) -> Result<BlobPage, BlobMaintenanceError>;
    /// False until the object contract is proven; callers record a typed blocked state.
    fn deletion_supported(&self) -> bool;
}
