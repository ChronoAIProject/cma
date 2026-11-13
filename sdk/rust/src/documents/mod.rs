//! Persistent documents with explicit per-request policy and immutable sources.
mod types;
use crate::{Client, Error, IdempotencyKey, PageOptions, Result, http::Request};
use reqwest::{Method, StatusCode, header};
use serde::{Serialize, de::DeserializeOwned};
pub use types::*;

/// Document APIs for one authenticated client; no grant is retained by the SDK.
pub struct Documents<'a> {
    client: &'a Client,
}
impl Client {
    /// Independent per-user document registry and authenticated sharing.
    pub fn documents(&self) -> Documents<'_> {
        Documents { client: self }
    }
}
impl Documents<'_> {
    /// List owned or explicitly shared documents; link tokens never expose inventory.
    pub async fn list(&self, options: &DocumentListOptions) -> Result<DocumentPage<Document>> {
        let mut query = Vec::new();
        if let Some(limit) = options.limit {
            query.push(("limit", limit.to_string()));
        }
        for (name, value) in [
            ("cursor", &options.cursor),
            ("scope", &options.scope),
            ("q", &options.q),
        ] {
            if let Some(value) = value {
                query.push((name, value.clone()));
            }
        }
        self.get(&["documents"], &query, None).await
    }
    /// Publish owned Markdown with a permanent idempotency key.
    pub async fn create(&self, body: &CreateDocument, key: &IdempotencyKey) -> Result<Document> {
        self.write(Method::POST, &["documents"], body, key, None)
            .await
    }
    /// Read metadata under current ownership, recipient or link authority.
    pub async fn read(&self, id: &str, token: Option<&str>) -> Result<Document> {
        self.get(&["documents", id], &[], token).await
    }
    /// Compare-and-swap source or title; revisions remain independently readable.
    pub async fn update(
        &self,
        id: &str,
        body: &UpdateDocument,
        key: &IdempotencyKey,
    ) -> Result<Document> {
        self.write(Method::PATCH, &["documents", id], body, key, None)
            .await
    }
    /// Permanently delete an owned document; exact retries retain the original receipt.
    pub async fn delete(
        &self,
        id: &str,
        body: &DeleteDocument,
        key: &IdempotencyKey,
    ) -> Result<DocumentDeletion> {
        if body.expected_revision == 0 {
            return Err(Error::validation("expected_revision must be positive"));
        }
        self.write(Method::DELETE, &["documents", id], body, key, None)
            .await
    }
    /// Read one immutable source; omission selects the current source revision.
    pub async fn source(
        &self,
        id: &str,
        revision: Option<u64>,
        token: Option<&str>,
    ) -> Result<DocumentSource> {
        let query = revision
            .map(|r| vec![("revision", r.to_string())])
            .unwrap_or_default();
        self.get(&["documents", id, "source"], &query, token).await
    }
    /// Read bounded immutable source revision metadata.
    pub async fn revisions(
        &self,
        id: &str,
        options: &PageOptions,
        token: Option<&str>,
    ) -> Result<DocumentPage<DocumentRevision>> {
        self.get(&["documents", id, "revisions"], &page_query(options), token)
            .await
    }
    /// Owner-only explicit recipient inventory.
    pub async fn shares(&self, id: &str) -> Result<Vec<DocumentShare>> {
        self.get(&["documents", id, "shares"], &[], None).await
    }
    /// Create or update a direct recipient's current permission.
    pub async fn grant(
        &self,
        id: &str,
        subject: &str,
        body: &GrantDocument,
        key: &IdempotencyKey,
    ) -> Result<Document> {
        self.write(
            Method::PUT,
            &["documents", id, "shares", subject],
            body,
            key,
            None,
        )
        .await
    }
    /// Revoke one direct recipient without altering other grants or links.
    pub async fn revoke(
        &self,
        id: &str,
        subject: &str,
        body: &DocumentRevisionRequest,
        key: &IdempotencyKey,
    ) -> Result<Document> {
        self.write(
            Method::DELETE,
            &["documents", id, "shares", subject],
            body,
            key,
            None,
        )
        .await
    }
    /// Owner-only share-link metadata; tokens are never returned.
    pub async fn links(&self, id: &str) -> Result<Vec<DocumentLink>> {
        self.get(&["documents", id, "share-links"], &[], None).await
    }
    /// Register the digest of a caller-generated link token.
    pub async fn create_link(
        &self,
        id: &str,
        body: &CreateDocumentLink,
        key: &IdempotencyKey,
    ) -> Result<DocumentMutation> {
        self.write(
            Method::POST,
            &["documents", id, "share-links"],
            body,
            key,
            None,
        )
        .await
    }
    /// Revoke a link under the observed aggregate revision.
    pub async fn revoke_link(
        &self,
        id: &str,
        link: &str,
        body: &DocumentRevisionRequest,
        key: &IdempotencyKey,
    ) -> Result<Document> {
        self.write(
            Method::DELETE,
            &["documents", id, "share-links", link],
            body,
            key,
            None,
        )
        .await
    }
    /// Read comments under current document access.
    pub async fn comments(
        &self,
        id: &str,
        options: &PageOptions,
        token: Option<&str>,
    ) -> Result<DocumentPage<DocumentComment>> {
        self.get(&["documents", id, "comments"], &page_query(options), token)
            .await
    }
    /// Admit a comment atomically with the current comment grant.
    pub async fn comment(
        &self,
        id: &str,
        body: &CreateDocumentComment,
        key: &IdempotencyKey,
        token: Option<&str>,
    ) -> Result<DocumentMutation> {
        self.write(
            Method::POST,
            &["documents", id, "comments"],
            body,
            key,
            token,
        )
        .await
    }
    /// Remove one's own comment, or any comment as owner.
    pub async fn remove_comment(
        &self,
        id: &str,
        comment: &str,
        body: &DocumentRevisionRequest,
        key: &IdempotencyKey,
        token: Option<&str>,
    ) -> Result<Document> {
        self.write(
            Method::DELETE,
            &["documents", id, "comments", comment],
            body,
            key,
            token,
        )
        .await
    }
    async fn get<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
        token: Option<&str>,
    ) -> Result<T> {
        let mut request = Request::get(segments);
        request.query = query;
        add_token(&mut request, token)?;
        self.client.request(request).await
    }
    async fn write<T: DeserializeOwned, B: Serialize>(
        &self,
        method: Method,
        segments: &[&str],
        body: &B,
        key: &IdempotencyKey,
        token: Option<&str>,
    ) -> Result<T> {
        let status = if method == Method::POST {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        let mut request = Request::command(method, segments, Some(body), key, status)?;
        add_token(&mut request, token)?;
        self.client.request(request).await
    }
}
fn add_token(request: &mut Request<'_>, token: Option<&str>) -> Result<()> {
    if let Some(token) = token {
        request.headers.insert(
            "x-cma-document-share-token",
            header::HeaderValue::from_str(token)
                .map_err(|_| Error::validation("invalid document share header"))?,
        );
    }
    Ok(())
}
fn page_query(options: &PageOptions) -> Vec<(&str, String)> {
    let mut query = Vec::new();
    if let Some(limit) = options.limit {
        query.push(("limit", limit.to_string()));
    }
    if let Some(cursor) = &options.cursor {
        query.push(("cursor", cursor.clone()));
    }
    query
}
