use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
pub const MAX_DOCUMENT_PAGE: u16 = 50;
pub const MAX_REVISIONS: usize = 256;
pub const MAX_COMMENTS: usize = 256;
pub const MAX_RECEIPTS: usize = 1024;
pub const MAX_GRANTS: usize = 128;
pub const MAX_LINKS: usize = 32;
pub(super) const MAX_METADATA_BYTES: usize = 12 * 1024 * 1024;
// One bounded receipt per live grant/link remains available for owner revocation.
// Normal writes never consume this reserve, including when metadata fills first.
pub(super) const REVOCATION_RECEIPT_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DocumentError {
    #[error("account deletion is in progress")]
    AccountDeletionInProgress,
    #[error("share recipient is unavailable")]
    ShareRecipientUnavailable,
    #[error("Choose a CMA user with a valid recipient identifier")]
    InvalidShareRecipient,
    #[error("Recipient lookup is temporarily unavailable. Try again.")]
    RecipientDirectoryUnavailable,
    #[error("Document request is invalid")]
    Invalid,
    #[error("Document is unavailable to this caller")]
    NotFound,
    #[error("Document revision changed")]
    Conflict,
    #[error("Idempotency key identifies another request")]
    IdempotencyConflict,
    #[error("Document capacity reached")]
    Capacity,
    #[error("Document storage is unavailable")]
    Unavailable,
    #[error("Document integrity verification failed")]
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentPermission {
    Read,
    Comment,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentAccess {
    Owner,
    Read,
    Comment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentShareRecipient {
    pub subject: crate::SubjectId,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentView {
    pub id: String,
    pub owner_subject: String,
    pub title: String,
    pub revision: u64,
    pub source_revision: u64,
    pub sha256: String,
    pub size_bytes: u64,
    pub created_at: String,
    pub updated_at: String,
    pub origin_agent_id: Option<String>,
    pub access: DocumentAccess,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentRevision {
    pub revision: u64,
    pub title: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub created_at: String,
    pub author_subject: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSource {
    pub id: String,
    pub revision: u64,
    pub markdown: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentComment {
    pub id: String,
    pub author_subject: String,
    pub body: String,
    pub document_revision: u64,
    pub created_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentShare {
    pub subject: String,
    pub permission: DocumentPermission,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentLink {
    pub id: String,
    pub permission: DocumentPermission,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentPage<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDocument {
    pub title: String,
    pub markdown: String,
    pub origin_agent_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDocument {
    pub expected_revision: u64,
    pub title: Option<String>,
    pub markdown: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantDocument {
    pub expected_revision: u64,
    pub permission: DocumentPermission,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDocumentLink {
    pub expected_revision: u64,
    pub permission: DocumentPermission,
    /// Caller-generated 256-bit random value. Only its digest is retained.
    pub token: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDocumentComment {
    pub body: String,
    pub document_revision: Option<u64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentQuery {
    pub limit: Option<u16>,
    pub cursor: Option<String>,
    pub q: Option<String>,
    pub scope: Option<String>,
}
impl DocumentQuery {
    pub fn limit(&self) -> usize {
        usize::from(self.limit.unwrap_or(25))
    }
    pub fn validate(&self) -> Result<(), DocumentError> {
        if self.limit() == 0
            || self.limit() > usize::from(MAX_DOCUMENT_PAGE)
            || self.cursor.as_ref().is_some_and(|v| !valid_id(v, "doc_"))
            || self
                .q
                .as_ref()
                .is_some_and(|v| v.len() > 160 || v.chars().any(char::is_control))
            || self
                .scope
                .as_ref()
                .is_some_and(|v| !matches!(v.as_str(), "all" | "owned" | "shared"))
        {
            return Err(DocumentError::Invalid);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRevision {
    pub metadata: DocumentRevision,
    pub object_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredLink {
    pub metadata: DocumentLink,
    pub token_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentReceipt {
    pub fingerprint: String,
    pub outcome: DocumentMutation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMutation {
    pub document: DocumentView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<DocumentComment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<DocumentLink>,
}
/// Bounded aggregate; all policy edits and comments conflict on the same revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredDocument {
    /// Verified runtime publication authority; never accepted from a REST body.
    #[serde(default)]
    pub runtime_author_agent_id: Option<String>,
    pub metadata: DocumentView,
    pub title_search: String,
    pub revisions: Vec<StoredRevision>,
    pub shares: Vec<DocumentShare>,
    pub links: Vec<StoredLink>,
    pub comments: Vec<DocumentComment>,
    pub receipts: BTreeMap<String, DocumentReceipt>,
    pub creation_fingerprint: String,
}

/// Small policy projection for inventory queries; excludes history and receipts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentListing {
    pub metadata: DocumentView,
    pub shares: Vec<DocumentShare>,
}
impl DocumentListing {
    pub fn view(mut self, subject: &str) -> Result<DocumentView, DocumentError> {
        if !valid_id(&self.metadata.id, "doc_")
            || !valid_subject(&self.metadata.owner_subject)
            || !valid_title(&self.metadata.title)
            || self.shares.len() > MAX_GRANTS
        {
            return Err(DocumentError::Corrupt);
        }
        self.metadata.access = if self.metadata.owner_subject == subject {
            DocumentAccess::Owner
        } else {
            match self
                .shares
                .iter()
                .find(|s| s.subject == subject)
                .map(|s| s.permission)
            {
                Some(DocumentPermission::Read) => DocumentAccess::Read,
                Some(DocumentPermission::Comment) => DocumentAccess::Comment,
                None => return Err(DocumentError::NotFound),
            }
        };
        Ok(self.metadata)
    }
}

#[async_trait::async_trait]
pub trait DocumentStore: Send + Sync {
    async fn get(&self, id: &str) -> Result<Option<StoredDocument>, DocumentError>;
    /// Read the immutable owner-deletion receipt for this identity.
    async fn tombstone(
        &self,
        _id: &str,
    ) -> Result<Option<super::DocumentTombstone>, DocumentError> {
        Ok(None)
    }
    /// Atomically fence the live owner/revision and retain the deletion receipt.
    async fn delete(
        &self,
        _owner: &str,
        _tombstone: &super::DocumentTombstone,
    ) -> Result<bool, DocumentError> {
        Err(DocumentError::Unavailable)
    }
    /// Insert exactly once; false means the deterministic identity already exists.
    async fn insert(&self, record: &StoredDocument) -> Result<bool, DocumentError>;
    /// Publish a new Document with its reserved source on stores that track publication.
    /// False asks the caller to resolve the response from the persisted creation.
    async fn insert_source(
        &self,
        record: &StoredDocument,
        _source: &super::DocumentSourceWrite,
    ) -> Result<bool, DocumentError> {
        self.insert(record).await
    }
    /// Replace only the same owner/id at the exact revision; never upsert.
    async fn compare_exchange(
        &self,
        expected_revision: u64,
        record: &StoredDocument,
    ) -> Result<bool, DocumentError>;
    /// Publish a Markdown edit with its reserved source and exact operation receipt.
    /// False asks the caller to reload the aggregate and check its durable receipt.
    async fn compare_exchange_source(
        &self,
        expected_revision: u64,
        record: &StoredDocument,
        _source: &super::DocumentSourceWrite,
        _receipt_key: &str,
    ) -> Result<bool, DocumentError> {
        self.compare_exchange(expected_revision, record).await
    }
    /// Persist a grant while checking the recipient at the same atomic boundary.
    async fn compare_exchange_grant(
        &self,
        expected_revision: u64,
        record: &StoredDocument,
        _recipient: &str,
    ) -> Result<bool, DocumentError> {
        self.compare_exchange(expected_revision, record).await
    }
    /// Owner or explicit recipient grants only; links never make a document searchable.
    /// Return ordered limit+1 matching rows after the cursor.
    async fn list(
        &self,
        subject: &str,
        query: &DocumentQuery,
    ) -> Result<Vec<DocumentListing>, DocumentError>;
}
#[async_trait::async_trait]
pub trait DocumentBlobStore: Send + Sync {
    /// Write immutable content at the server-generated content-addressed key.
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError>;
    async fn put_source(
        &self,
        write: &super::DocumentSourceWrite,
        bytes: Vec<u8>,
    ) -> Result<(), DocumentError> {
        if write.generation() != 1 {
            return Err(DocumentError::Unavailable);
        }
        self.put(&write.source.object_key(), bytes).await
    }
    /// Implementations must bound reads at MAX_DOCUMENT_BYTES and never follow caller URLs.
    async fn get(&self, key: &str) -> Result<Vec<u8>, DocumentError>;
}
pub fn valid_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|v| {
        v.len() == 32
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
pub fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
pub(crate) fn fingerprint(value: &impl Serialize) -> Result<String, DocumentError> {
    Ok(digest(
        &serde_json::to_vec(value).map_err(|_| DocumentError::Invalid)?,
    ))
}
pub(crate) fn identity(prefix: &str, parts: &[&str]) -> String {
    let value = serde_json::to_vec(parts).expect("string tuple serializes");
    format!(
        "{prefix}{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, &value).simple()
    )
}
pub(crate) fn valid_subject(v: &str) -> bool {
    !v.is_empty() && v.len() <= 255 && !v.chars().any(char::is_control)
}
pub(crate) fn valid_title(v: &str) -> bool {
    !v.trim().is_empty() && v.len() <= 240 && !v.chars().any(char::is_control)
}
pub(crate) fn valid_markdown(v: &str) -> bool {
    !v.is_empty() && v.len() <= MAX_DOCUMENT_BYTES && !v.contains('\0')
}
