//! Handler schema projections; requests are decoded into the core's bounded commands.
#![allow(dead_code)]
use utoipa::ToSchema;
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DocumentPermission {
    Read,
    Comment,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DocumentAccess {
    Owner,
    Read,
    Comment,
}
#[derive(ToSchema)]
pub(crate) struct DocumentView {
    id: String,
    owner_subject: String,
    title: String,
    revision: u64,
    source_revision: u64,
    sha256: String,
    size_bytes: u64,
    created_at: String,
    updated_at: String,
    origin_agent_id: Option<String>,
    access: DocumentAccess,
}
#[derive(ToSchema)]
pub(crate) struct DocumentDeletionReceipt {
    id: String,
    revision: u64,
    deleted_at: String,
}
#[derive(ToSchema)]
pub(crate) struct DocumentPage {
    items: Vec<DocumentView>,
    next: Option<String>,
}
#[derive(ToSchema)]
pub(crate) struct DocumentSource {
    id: String,
    revision: u64,
    markdown: String,
}
#[derive(ToSchema)]
pub(crate) struct DocumentRevision {
    revision: u64,
    title: String,
    sha256: String,
    size_bytes: u64,
    created_at: String,
    author_subject: String,
}
#[derive(ToSchema)]
pub(crate) struct DocumentRevisionPage {
    items: Vec<DocumentRevision>,
    next: Option<String>,
}
#[derive(ToSchema)]
pub(crate) struct DocumentComment {
    id: String,
    author_subject: String,
    body: String,
    document_revision: u64,
    created_at: String,
}
#[derive(ToSchema)]
pub(crate) struct DocumentCommentPage {
    items: Vec<DocumentComment>,
    next: Option<String>,
}
#[derive(ToSchema)]
pub(crate) struct DocumentShare {
    subject: String,
    permission: DocumentPermission,
}
#[derive(ToSchema)]
pub(crate) struct DocumentShareRecipient {
    subject: String,
    display_name: Option<String>,
}
#[derive(ToSchema)]
pub(crate) struct DocumentLink {
    id: String,
    permission: DocumentPermission,
    created_at: String,
}
#[derive(ToSchema)]
pub(crate) struct DocumentMutation {
    document: DocumentView,
    comment: Option<DocumentComment>,
    link: Option<DocumentLink>,
}
#[derive(ToSchema)]
pub(crate) struct CreateDocumentRequest {
    title: String,
    markdown: String,
    origin_agent_id: Option<String>,
}
#[derive(ToSchema)]
pub(crate) struct UpdateDocumentRequest {
    expected_revision: u64,
    title: Option<String>,
    markdown: Option<String>,
}
#[derive(ToSchema)]
pub(crate) struct GrantDocumentRequest {
    expected_revision: u64,
    permission: DocumentPermission,
}
#[derive(ToSchema)]
pub(crate) struct CreateDocumentLinkRequest {
    expected_revision: u64,
    permission: DocumentPermission,
    token: String,
}
#[derive(ToSchema)]
pub(crate) struct CreateDocumentCommentRequest {
    body: String,
    document_revision: Option<u64>,
}
#[derive(serde::Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentRevisionRequest {
    pub expected_revision: u64,
}
