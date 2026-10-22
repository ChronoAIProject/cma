//! Wire types for independent durable documents.
use serde::{Deserialize, Serialize};
/// Effective document permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentPermission {
    /// Read access.
    Read,
    /// Comment access.
    Comment,
}
/// Effective document permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentAccess {
    /// Owner access.
    Owner,
    /// Read access.
    Read,
    /// Comment access.
    Comment,
}
/// Document API response projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    /// Stable document identity.
    pub id: String,
    /// Owning authenticated subject.
    pub owner_subject: String,
    /// Current display title.
    pub title: String,
    /// Aggregate revision used for compare-and-swap writes.
    pub revision: u64,
    /// Current immutable source revision.
    pub source_revision: u64,
    /// Digest of current UTF-8 source bytes.
    pub sha256: String,
    /// Current source byte count.
    pub size_bytes: u64,
    /// Initial creation timestamp.
    pub created_at: String,
    /// Latest mutation timestamp.
    pub updated_at: String,
    /// Informational origin; owner-supplied labels carry no authority.
    pub origin_agent_id: Option<String>,
    /// Effective access for this request.
    pub access: DocumentAccess,
}
/// Document API response projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentRevision {
    /// Immutable source revision.
    pub revision: u64,
    /// Title at that revision.
    pub title: String,
    /// Source content digest.
    pub sha256: String,
    /// Source UTF-8 size.
    pub size_bytes: u64,
    /// Revision creation time.
    pub created_at: String,
    /// Author subject.
    pub author_subject: String,
}
/// Document API response projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSource {
    /// Requested document.
    pub id: String,
    /// Returned source revision.
    pub revision: u64,
    /// Authorized Markdown source.
    pub markdown: String,
}
/// Document API response projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentComment {
    /// Stable comment identity.
    pub id: String,
    /// Comment author.
    pub author_subject: String,
    /// Comment text.
    pub body: String,
    /// Source revision this comment references.
    pub document_revision: u64,
    /// Admission timestamp.
    pub created_at: String,
}
/// Document API response projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentShare {
    /// Recipient subject.
    pub subject: String,
    /// Recipient access.
    pub permission: DocumentPermission,
}
/// Document API response projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentLink {
    /// Link identity; no token is returned.
    pub id: String,
    /// Link access; authentication still required.
    pub permission: DocumentPermission,
    /// Link creation time.
    pub created_at: String,
}
/// Document API response projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentMutation {
    /// Document metadata after admission.
    pub document: Document,
    /// Created comment, if applicable.
    pub comment: Option<DocumentComment>,
    /// Created link, if applicable.
    pub link: Option<DocumentLink>,
}
/// Bounded command for a persistent document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDocument {
    /// Display title.
    pub title: String,
    /// Source Markdown, at most 1 MiB.
    pub markdown: String,
    /// Optional informational originating Agent.
    pub origin_agent_id: Option<String>,
}
/// Bounded command for a persistent document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDocument {
    /// Last-observed aggregate revision.
    pub expected_revision: u64,
    /// Replacement title, if supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Replacement source, if supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
}
/// Bounded command for a persistent document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantDocument {
    /// Last-observed aggregate revision.
    pub expected_revision: u64,
    /// Recipient access.
    pub permission: DocumentPermission,
}
/// Bounded command for a persistent document.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDocumentLink {
    /// Last-observed aggregate revision.
    pub expected_revision: u64,
    /// Authenticated link access.
    pub permission: DocumentPermission,
    /// Caller-generated 256-bit CSPRNG value encoded as lowercase hex.
    pub token: String,
}
/// Bounded command for a persistent document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDocumentComment {
    /// Comment text, at most 8 KiB.
    pub body: String,
    /// Source revision; omission selects the current source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_revision: Option<u64>,
}
/// Bounded command for a persistent document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentRevisionRequest {
    /// Last-observed aggregate revision.
    pub expected_revision: u64,
}
/// Bounded inventory page. An absent next cursor marks the final page.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DocumentPage<T> {
    /// Returned values.
    pub items: Vec<T>,
    /// Continuation cursor.
    pub next: Option<String>,
}
/// Query for the owner's and explicit recipient's registry.
#[derive(Debug, Clone, Default)]
pub struct DocumentListOptions {
    /// Page size, one through fifty.
    pub limit: Option<u16>,
    /// Previous page cursor.
    pub cursor: Option<String>,
    /// Literal title substring.
    pub q: Option<String>,
    /// all, owned, or shared.
    pub scope: Option<String>,
}
