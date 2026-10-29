//! Narrow binding-event file publication and immutable supplementary input.
use crate::AuthenticatedResourceCaller;
use async_trait::async_trait;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use layer2_core::sandbox::{
    agent_attachments::{AgentAttachment, MAX_ATTACHMENT_BYTES},
    agent_io::AgentResponseReading,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

#[derive(Debug, Clone, Deserialize)]
pub struct BindingMediaPath {
    pub binding_id: String,
    pub conversation_id: String,
    pub event_id: String,
    pub attachment_index: u8,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingMediaUploadParams {
    pub name: String,
    pub sha256: String,
    pub gateway_binding_generation: Option<u64>,
    pub event_expires_at: i64,
}
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BindingMediaSupplement {
    pub gateway_binding_generation: Option<u64>,
    pub event_expires_at: i64,
    pub artifact: Value,
    pub derived_text: String,
}
#[derive(Debug, Clone, Copy)]
pub enum BindingMediaError {
    Invalid,
    Forbidden,
    Conflict,
    NotReady,
    NotFound,
    TooLarge,
    Unavailable,
}
#[async_trait]
pub trait BindingMediaUpload: Send {
    async fn upload(self: Box<Self>, bytes: Vec<u8>) -> Result<AgentAttachment, BindingMediaError>;
}
#[async_trait]
pub trait BindingMediaService: Send + Sync {
    async fn attachment(
        &self,
        caller: AuthenticatedResourceCaller,
        path: BindingMediaPath,
    ) -> Result<AgentAttachment, BindingMediaError>;
    async fn prepare_upload(
        self: Arc<Self>,
        caller: AuthenticatedResourceCaller,
        path: BindingMediaPath,
        params: BindingMediaUploadParams,
        media_type: String,
        size_bytes: u64,
    ) -> Result<Box<dyn BindingMediaUpload>, BindingMediaError>;
    async fn supplement(
        &self,
        caller: AuthenticatedResourceCaller,
        path: BindingMediaPath,
        body: BindingMediaSupplement,
    ) -> Result<AgentResponseReading, BindingMediaError>;
}
struct MediaState {
    service: Arc<dyn BindingMediaService>,
    uploads: Semaphore,
}
pub fn binding_media_router(service: Arc<dyn BindingMediaService>) -> Router {
    Router::new()
        .route("/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/attachment", post(upload).get(attachment))
        .route("/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/supplement", post(supplement))
        .with_state(Arc::new(MediaState { service, uploads: Semaphore::new(2) }))
}
#[utoipa::path(post,operation_id="upload_binding_event_attachment",path="/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/attachment",tag="provider bindings",
    params(("binding_id"=String,Path),("conversation_id"=String,Path),("event_id"=String,Path),("attachment_index"=u8,Path),("name"=String,Query),("sha256"=String,Query),("event_expires_at"=i64,Query),("gateway_binding_generation"=Option<u64>,Query)),
    request_body(content=String,content_type="application/octet-stream"),responses((status=201,body=Object),(status=400),(status=403),(status=409),(status=413),(status=429),(status=503)))]
async fn upload(
    State(state): State<Arc<MediaState>>,
    caller: AuthenticatedResourceCaller,
    Path(path): Path<BindingMediaPath>,
    params: Result<Query<BindingMediaUploadParams>, axum::extract::rejection::QueryRejection>,
    request: axum::extract::Request,
) -> Response {
    let Query(params) = match params {
        Ok(value) => value,
        Err(_) => return error(BindingMediaError::Invalid),
    };
    let (parts, body) = request.into_parts();
    if parts.headers.contains_key(header::CONTENT_ENCODING) {
        return error(BindingMediaError::Invalid);
    }
    let header_value = |name: axum::http::HeaderName| {
        let mut all = parts.headers.get_all(name).iter();
        let result = all.next()?.to_str().ok()?;
        all.next().is_none().then_some(result)
    };
    let Some(media_type) = header_value(header::CONTENT_TYPE) else {
        return error(BindingMediaError::Invalid);
    };
    let Some(size) = header_value(header::CONTENT_LENGTH).and_then(|v| v.parse::<u64>().ok())
    else {
        return error(BindingMediaError::Invalid);
    };
    if size > MAX_ATTACHMENT_BYTES as u64 {
        return error(BindingMediaError::TooLarge);
    }
    let admission = match state
        .service
        .clone()
        .prepare_upload(caller, path, params, media_type.into(), size)
        .await
    {
        Ok(value) => value,
        Err(value) => return error(value),
    };
    let Ok(_permit) = state.uploads.try_acquire() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::CACHE_CONTROL, "private, no-store")],
            Json(json!({"error":"media_upload_capacity"})),
        )
            .into_response();
    };
    let bytes = match tokio::time::timeout(
        Duration::from_secs(45),
        axum::body::to_bytes(body, MAX_ATTACHMENT_BYTES),
    )
    .await
    {
        Ok(Ok(bytes)) if bytes.len() as u64 == size => bytes.to_vec(),
        _ => return error(BindingMediaError::Invalid),
    };
    match admission.upload(bytes).await {
        Ok(receipt) => (
            StatusCode::CREATED,
            [(header::CACHE_CONTROL, "private, no-store")],
            Json(receipt),
        )
            .into_response(),
        Err(value) => error(value),
    }
}
#[utoipa::path(post,operation_id="admit_binding_event_media_supplement",path="/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/supplement",tag="provider bindings",
    params(("binding_id"=String,Path),("conversation_id"=String,Path),("event_id"=String,Path),("attachment_index"=u8,Path)),request_body=BindingMediaSupplement,
    responses((status=202,body=Object),(status=400),(status=403),(status=409),(status=503)))]
async fn supplement(
    State(state): State<Arc<MediaState>>,
    caller: AuthenticatedResourceCaller,
    Path(path): Path<BindingMediaPath>,
    Json(body): Json<BindingMediaSupplement>,
) -> Response {
    match state.service.supplement(caller, path, body).await {
        Ok(reading) => {
            let view = reading.command;
            let response_id = view.receipt.response_id.as_ref().map(ToString::to_string);
            (
                StatusCode::ACCEPTED,
                [(header::CACHE_CONTROL, "private, no-store")],
                Json(json!({
                    "command_id":view.receipt.command_id,"response_id":response_id,
                    "response_url":response_id.as_ref().map(|id|format!("/api/v2/responses/{id}")),
                    "status":view.status,"queue_deadline":view.queue_deadline,"metadata":reading.metadata
                })),
            )
                .into_response()
        }
        Err(value) => error(value),
    }
}
fn error(value: BindingMediaError) -> Response {
    let (status, code) = match value {
        BindingMediaError::Invalid => (StatusCode::BAD_REQUEST, "invalid_media_continuation"),
        BindingMediaError::Forbidden => (StatusCode::FORBIDDEN, "media_continuation_refused"),
        BindingMediaError::Conflict => (StatusCode::CONFLICT, "media_continuation_conflict"),
        BindingMediaError::NotFound => (StatusCode::NOT_FOUND, "media_attachment_not_found"),
        BindingMediaError::NotReady => (StatusCode::CONFLICT, "media_agent_not_ready"),
        BindingMediaError::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "media_too_large"),
        BindingMediaError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "media_continuation_unavailable",
        ),
    };
    (
        status,
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(json!({"error":code})),
    )
        .into_response()
}

#[utoipa::path(get,operation_id="get_binding_event_attachment",path="/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/attachment",tag="provider bindings",
    params(("binding_id"=String,Path),("conversation_id"=String,Path),("event_id"=String,Path),("attachment_index"=u8,Path)),responses((status=200,body=Object),(status=403),(status=404),(status=503)))]
async fn attachment(
    State(state): State<Arc<MediaState>>,
    caller: AuthenticatedResourceCaller,
    Path(path): Path<BindingMediaPath>,
) -> Response {
    match state.service.attachment(caller, path).await {
        Ok(receipt) => (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "private, no-store")],
            Json(receipt),
        )
            .into_response(),
        Err(value) => error(value),
    }
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(upload, supplement, attachment),
    components(schemas(BindingMediaSupplement))
)]
struct MediaApi;
pub(super) fn document() -> utoipa::openapi::OpenApi {
    <MediaApi as utoipa::OpenApi>::openapi()
}

#[cfg(test)]
mod tests {
    #[test]
    fn media_continuation_operations_are_in_the_authenticated_public_document() {
        let document = serde_json::to_value(super::super::openapi::document()).unwrap();
        let base = "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}";
        for (suffix, method, success) in [
            ("attachment", "post", "201"),
            ("attachment", "get", "200"),
            ("supplement", "post", "202"),
        ] {
            let operation = &document["paths"][format!("{base}/{suffix}")][method];
            assert!(operation["responses"][success].is_object());
            assert!(operation["security"]
                .as_array()
                .is_some_and(|v| !v.is_empty()));
        }
        assert!(document["components"]["schemas"]["BindingMediaSupplement"].is_object());
    }
}
