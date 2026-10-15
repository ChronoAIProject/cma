//! Bounded owner-authorized binary uploads; file bytes never enter the activity log.
use super::idempotency::IdempotencyKey;
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use futures_util::StreamExt as _;
use layer2_core::{
    sandbox::agent_attachments::{
        AgentAttachment, AgentAttachmentService, AttachmentError, AttachmentMetadata,
        MAX_ATTACHMENT_BYTES,
    },
    AgentId,
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use utoipa::ToSchema;

pub(crate) struct AttachmentState {
    service: Arc<AgentAttachmentService>,
    uploads: Semaphore,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttachmentParams {
    name: String,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct AgentAttachmentView {
    id: String,
    agent_id: String,
    name: String,
    path: String,
    size_bytes: u64,
    media_type: String,
    sha256: String,
}
impl From<AgentAttachment> for AgentAttachmentView {
    fn from(a: AgentAttachment) -> Self {
        Self {
            id: a.id,
            agent_id: a.agent_id.to_string(),
            name: a.name,
            path: a.path,
            size_bytes: a.size_bytes,
            media_type: a.media_type,
            sha256: a.sha256,
        }
    }
}
pub fn agent_attachments_router(service: Arc<AgentAttachmentService>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/attachments", post(upload))
        .layer(axum::middleware::from_fn(private_headers))
        .with_state(Arc::new(AttachmentState {
            service,
            uploads: Semaphore::new(2),
        }))
}
async fn private_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
}
#[utoipa::path(post, path = "/api/v1/agents/{id}/attachments", tag = "agents",
    params(("id" = String, Path), ("name" = String, Query), ("Idempotency-Key" = String, Header)),
    request_body(content = String, content_type = "application/octet-stream"),
    responses((status = 201, body = AgentAttachmentView), (status = 400, description = "Invalid upload metadata or incomplete body"), (status = 404, description = "Owner Agent not found"), (status = 409, description = "Agent unavailable or upload conflict"), (status = 413, description = "Maximum 20 MiB per file"), (status = 429, description = "Upload capacity busy"), (status = 503, description = "Runtime upload unavailable")))]
pub(crate) async fn upload(
    State(state): State<Arc<AttachmentState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    key: IdempotencyKey,
    params: Result<Query<AttachmentParams>, axum::extract::rejection::QueryRejection>,
    request: axum::extract::Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let key = match key.into_required_value() {
        Ok(k) => k,
        Err(e) => return e.into_response(),
    };
    let Query(params) = match params {
        Ok(p) => p,
        Err(_) => return AttachmentProblem(AttachmentError::Invalid).into_response(),
    };
    let media = match single_header(&headers, header::CONTENT_TYPE) {
        Ok(Some(v)) => v,
        Ok(None) => "application/octet-stream",
        Err(e) => return e.into_response(),
    };
    if headers.contains_key(header::CONTENT_ENCODING) {
        return AttachmentProblem(AttachmentError::Invalid).into_response();
    }
    let expected = match single_header(&headers, header::CONTENT_LENGTH) {
        Ok(Some(v)) => match v.parse::<usize>() {
            Ok(n) if n <= MAX_ATTACHMENT_BYTES => Some(n),
            Ok(_) => return AttachmentProblem(AttachmentError::TooLarge).into_response(),
            Err(_) => return AttachmentProblem(AttachmentError::Invalid).into_response(),
        },
        Ok(None) => None,
        Err(e) => return e.into_response(),
    };
    let admission = match state
        .service
        .admit(
            &caller,
            resource.as_ref().map(|r| &r.context),
            resource
                .as_ref()
                .and_then(|r| r.credential.as_ref())
                .map(DelegatedCredential::value),
            &AgentId(id),
            AttachmentMetadata {
                key: &key,
                name: &params.name,
                media_type: media,
            },
        )
        .await
    {
        Ok(a) => a,
        Err(e) => return AttachmentProblem(e).into_response(),
    };
    let Ok(_permit) = state.uploads.try_acquire() else {
        return super::problem_response(
            StatusCode::TOO_MANY_REQUESTS,
            super::problem_document(
                StatusCode::TOO_MANY_REQUESTS,
                "attachment_capacity",
                "Uploads are busy. Retry with the same Idempotency-Key.",
                "attachment_capacity",
            ),
        );
    };
    // Deliberately consume Body directly under this route's own hard bound. The
    // global JSON extractor limit stays 2 MiB for every ordinary endpoint.
    let bytes = match tokio::time::timeout(Duration::from_secs(45), read_body(body, expected)).await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => return e.into_response(),
        Err(_) => {
            return super::problem_response(
                StatusCode::REQUEST_TIMEOUT,
                super::problem_document(
                    StatusCode::REQUEST_TIMEOUT,
                    "attachment_timeout",
                    "Upload body did not complete within 45 seconds.",
                    "attachment_timeout",
                ),
            )
        }
    };
    match state.service.upload(admission, bytes).await {
        Ok(a) => (StatusCode::CREATED, Json(AgentAttachmentView::from(a))).into_response(),
        Err(e) => AttachmentProblem(e).into_response(),
    }
}
fn single_header(
    headers: &HeaderMap,
    name: header::HeaderName,
) -> Result<Option<&str>, AttachmentProblem> {
    let mut values = headers.get_all(name).iter();
    let value = values.next();
    if values.next().is_some() {
        return Err(AttachmentProblem(AttachmentError::Invalid));
    }
    value
        .map(|v| {
            v.to_str()
                .map_err(|_| AttachmentProblem(AttachmentError::Invalid))
        })
        .transpose()
}
async fn read_body(body: Body, expected: Option<usize>) -> Result<Vec<u8>, AttachmentProblem> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::with_capacity(expected.unwrap_or(0).min(MAX_ATTACHMENT_BYTES));
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AttachmentProblem(AttachmentError::Invalid))?;
        if chunk.len() > MAX_ATTACHMENT_BYTES.saturating_sub(bytes.len()) {
            return Err(AttachmentProblem(AttachmentError::TooLarge));
        }
        bytes.extend_from_slice(&chunk);
    }
    if expected.is_some_and(|n| n != bytes.len()) {
        return Err(AttachmentProblem(AttachmentError::Invalid));
    }
    Ok(bytes)
}
#[derive(Debug)]
struct AttachmentProblem(AttachmentError);
impl IntoResponse for AttachmentProblem {
    fn into_response(self) -> Response {
        let (status, code, detail) = match self.0 {
            AttachmentError::Access(e) => return super::resource_access_problem::ResourceProblem(e).into_response(),
            AttachmentError::Invalid => (StatusCode::BAD_REQUEST, "invalid_attachment", "Attachment metadata or body is invalid."),
            AttachmentError::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "attachment_too_large", "Each attachment may contain at most 20 MiB."),
            AttachmentError::NotReady => (StatusCode::CONFLICT, "attachment_agent_not_ready", "The Agent environment must be active before uploading attachments."),
            AttachmentError::Unsupported => (StatusCode::CONFLICT, "attachment_runtime_upgrade_required", "This retained runtime does not support persistent attachment uploads. Use an Agent created with a supported runtime."),
            AttachmentError::Conflict => (StatusCode::CONFLICT, "attachment_conflict", "The upload key or Agent environment changed. Retry an uncertain upload using the original key and file."),
            AttachmentError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "attachment_unavailable", "Upload is temporarily unavailable. Retry with the original key and file."),
        };
        super::problem_response(status, super::problem_document(status, code, detail, code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn binary_body_enforces_actual_size_and_rejects_partial_reads() {
        assert_eq!(
            read_body(Body::from(vec![0, 255, 7]), Some(3))
                .await
                .unwrap(),
            [0, 255, 7]
        );
        assert!(read_body(Body::from("partial"), Some(8)).await.is_err());
        assert!(
            read_body(Body::from(vec![0; MAX_ATTACHMENT_BYTES + 1]), None)
                .await
                .is_err()
        );
        let stream = futures_util::stream::iter([
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"partial")),
            Err(std::io::Error::other("closed")),
        ]);
        assert!(read_body(Body::from_stream(stream), None).await.is_err());
        assert!(read_body(Body::empty(), Some(0)).await.unwrap().is_empty());
    }
}
