//! Public document projections; durable policy and mutations belong to Layer 2.
mod reads;
mod shapes;
#[cfg(test)]
mod tests;
mod writes;
use super::idempotency::{IdempotencyKey, RequestBody};
use crate::AuthenticatedCaller;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::document_artifacts::{DocumentError, DocumentService, MAX_DOCUMENT_BYTES};
pub(crate) use shapes::*;
use std::sync::Arc;

pub fn documents_router(service: Arc<DocumentService>) -> Router {
    Router::new()
        .route("/api/v1/documents", get(reads::list).post(writes::create))
        .route(
            "/api/v1/documents/{id}",
            get(reads::read)
                .patch(writes::update)
                .delete(writes::delete),
        )
        .route("/api/v1/documents/{id}/source", get(reads::source))
        .route("/api/v1/documents/{id}/revisions", get(reads::revisions))
        .route("/api/v1/documents/{id}/shares", get(reads::shares))
        .route(
            "/api/v1/documents/{id}/share-recipients",
            get(reads::share_recipients),
        )
        .route(
            "/api/v1/documents/{id}/shares/{subject}",
            axum::routing::put(writes::grant).delete(writes::revoke),
        )
        .route(
            "/api/v1/documents/{id}/share-links",
            get(reads::links).post(writes::create_link),
        )
        .route(
            "/api/v1/documents/{id}/share-links/{link_id}",
            axum::routing::delete(writes::revoke_link),
        )
        .route(
            "/api/v1/documents/{id}/comments",
            get(reads::comments).post(writes::comment),
        )
        .route(
            "/api/v1/documents/{id}/comments/{comment_id}",
            axum::routing::delete(writes::remove_comment),
        )
        .layer(DefaultBodyLimit::max(MAX_DOCUMENT_BYTES * 6 + 16 * 1024))
        .layer(axum::middleware::from_fn(private_response))
        .with_state(service)
}
async fn private_response(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response =
        match tokio::time::timeout(std::time::Duration::from_secs(30), next.run(request)).await {
            Ok(response) => response,
            Err(_) => DocumentProblem(DocumentError::Unavailable).into_response(),
        };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
}
pub(super) fn share_token(headers: &HeaderMap) -> Result<Option<&str>, DocumentProblem> {
    let mut values = headers.get_all("x-cma-document-share-token").iter();
    let value = values
        .next()
        .map(|v| {
            v.to_str()
                .map_err(|_| DocumentProblem(DocumentError::Invalid))
        })
        .transpose()?;
    if values.next().is_some()
        || value.is_some_and(|v| {
            v.len() != 64
                || !v
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    {
        return Err(DocumentProblem(DocumentError::Invalid));
    }
    Ok(value)
}
fn json_body<T>(body: Result<RequestBody<T>, super::BodyProblem>) -> Result<T, super::BodyProblem> {
    body.and_then(|b| b.into_parts().0)
}
fn required_key(key: IdempotencyKey) -> Result<String, super::idempotency::IdempotencyProblem> {
    key.into_required_value()
}
fn result<T: serde::Serialize>(value: Result<T, DocumentError>, status: StatusCode) -> Response {
    match value {
        Ok(value) => (status, Json(value)).into_response(),
        Err(e) => DocumentProblem(e).into_response(),
    }
}
pub(crate) struct DocumentProblem(DocumentError);
impl From<DocumentError> for DocumentProblem {
    fn from(value: DocumentError) -> Self {
        Self(value)
    }
}
impl IntoResponse for DocumentProblem {
    fn into_response(self) -> Response {
        let (status, code) = match self.0 {
            DocumentError::AccountDeletionInProgress => {
                (StatusCode::FORBIDDEN, "account_deletion_in_progress")
            }
            DocumentError::ShareRecipientUnavailable => {
                (StatusCode::CONFLICT, "share_recipient_unavailable")
            }
            DocumentError::InvalidShareRecipient => {
                (StatusCode::BAD_REQUEST, "invalid_share_recipient")
            }
            DocumentError::RecipientDirectoryUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "recipient_directory_unavailable",
            ),
            DocumentError::Invalid => (StatusCode::BAD_REQUEST, "invalid_document_request"),
            DocumentError::NotFound => (StatusCode::NOT_FOUND, "document_not_found"),
            DocumentError::Conflict => (StatusCode::CONFLICT, "document_revision_conflict"),
            DocumentError::IdempotencyConflict => (StatusCode::CONFLICT, "idempotency_key_reused"),
            DocumentError::Capacity => (StatusCode::TOO_MANY_REQUESTS, "document_capacity"),
            DocumentError::Unavailable | DocumentError::Corrupt => {
                (StatusCode::SERVICE_UNAVAILABLE, "document_unavailable")
            }
        };
        super::problem_response(
            status,
            super::problem_document(status, code, &self.0.to_string(), code),
        )
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        reads::list,
        reads::read,
        reads::source,
        reads::revisions,
        reads::shares,
        reads::share_recipients,
        reads::links,
        reads::comments,
        writes::create,
        writes::update,
        writes::delete,
        writes::grant,
        writes::revoke,
        writes::create_link,
        writes::revoke_link,
        writes::comment,
        writes::remove_comment
    ),
    components(schemas(
        DocumentView,
        DocumentPage,
        DocumentSource,
        DocumentRevision,
        DocumentRevisionPage,
        DocumentComment,
        DocumentCommentPage,
        DocumentShare,
        DocumentShareRecipient,
        DocumentLink,
        DocumentMutation,
        DocumentDeletionReceipt,
        CreateDocumentRequest,
        UpdateDocumentRequest,
        GrantDocumentRequest,
        CreateDocumentLinkRequest,
        CreateDocumentCommentRequest,
        DocumentRevisionRequest,
        DocumentPermission,
        DocumentAccess
    ))
)]
struct DocumentApi;
pub(crate) fn document() -> utoipa::openapi::OpenApi {
    <DocumentApi as utoipa::OpenApi>::openapi()
}
