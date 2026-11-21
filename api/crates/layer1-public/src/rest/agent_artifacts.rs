use super::idempotency::{IdempotencyKey, RequestBody};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::resource_access::{ResourceAccessError, ResourceAccessStoreError};
use layer2_core::sandbox::agent_artifacts::{
    AgentArtifactMetadata, AgentArtifactService, ArtifactPageRequest, PublishAgentArtifact,
};
use layer2_core::AgentId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub(crate) struct AgentArtifactView {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub created_at: String,
}
impl From<AgentArtifactMetadata> for AgentArtifactView {
    fn from(value: AgentArtifactMetadata) -> Self {
        Self {
            id: value.id,
            agent_id: value.agent_id.to_string(),
            name: value.name,
            media_type: value.media_type,
            size_bytes: value.size_bytes,
            sha256: value.sha256,
            created_at: value.created_at,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct AgentArtifactPageView {
    pub items: Vec<AgentArtifactView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublishAgentArtifactRequest {
    pub name: String,
    pub media_type: String,
    pub content_base64: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArtifactParams {
    limit: Option<u16>,
    cursor: Option<String>,
}

pub fn agent_artifacts_router(service: Arc<AgentArtifactService>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/artifacts", get(list).post(publish))
        .route("/api/v1/agents/{id}/artifacts/{artifact_id}", get(read))
        .route(
            "/api/v1/agents/{id}/artifacts/{artifact_id}/download",
            get(download),
        )
        .layer(axum::middleware::from_fn(private_headers))
        .with_state(service)
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

#[utoipa::path(post, path = "/api/v1/agents/{id}/artifacts", tag = "agents",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = PublishAgentArtifactRequest, responses((status = 201, body = AgentArtifactView)))]
pub(crate) async fn publish(
    State(service): State<Arc<AgentArtifactService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<PublishAgentArtifactRequest>, super::BodyProblem>,
) -> Response {
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let body = match body.and_then(|body| body.into_parts().0) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let value = service
        .publish(
            &caller,
            resource.as_ref().map(|resource| &resource.context),
            resource
                .as_ref()
                .and_then(|resource| resource.credential.as_ref())
                .map(DelegatedCredential::value),
            &AgentId(id),
            &key,
            PublishAgentArtifact {
                name: body.name,
                media_type: body.media_type,
                content_base64: body.content_base64,
            },
        )
        .await;
    match value {
        Ok(value) => (StatusCode::CREATED, Json(AgentArtifactView::from(value))).into_response(),
        Err(error) => ArtifactProblem(error).into_response(),
    }
}
#[utoipa::path(get, path = "/api/v1/agents/{id}/artifacts", tag = "agents",
    params(("id" = String, Path), ("limit" = Option<u16>, Query), ("cursor" = Option<String>, Query)),
    responses((status = 200, body = AgentArtifactPageView)))]
pub(crate) async fn list(
    State(service): State<Arc<AgentArtifactService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    params: Result<Query<ArtifactParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<AgentArtifactPageView>, ArtifactProblem> {
    let Query(params) =
        params.map_err(|_| ArtifactProblem(ResourceAccessStoreError::Invalid.into()))?;
    let page = service
        .list(
            &caller,
            resource.as_ref().map(|resource| &resource.context),
            resource
                .as_ref()
                .and_then(|resource| resource.credential.as_ref())
                .map(DelegatedCredential::value),
            &AgentId(id),
            &ArtifactPageRequest {
                limit: params.limit.unwrap_or(50),
                cursor: params.cursor,
            },
        )
        .await?;
    Ok(Json(AgentArtifactPageView {
        items: page.items.into_iter().map(Into::into).collect(),
        next: page.next,
    }))
}
#[utoipa::path(get, path = "/api/v1/agents/{id}/artifacts/{artifact_id}", tag = "agents",
    params(("id" = String, Path), ("artifact_id" = String, Path)),
    responses((status = 200, body = AgentArtifactView)))]
pub(crate) async fn read(
    State(service): State<Arc<AgentArtifactService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path((id, artifact_id)): Path<(String, String)>,
) -> Result<Json<AgentArtifactView>, ArtifactProblem> {
    let (metadata, _) = service
        .read(
            &caller,
            resource.as_ref().map(|resource| &resource.context),
            resource
                .as_ref()
                .and_then(|resource| resource.credential.as_ref())
                .map(DelegatedCredential::value),
            &AgentId(id),
            &artifact_id,
        )
        .await?;
    Ok(Json(metadata.into()))
}
#[utoipa::path(get, path = "/api/v1/agents/{id}/artifacts/{artifact_id}/download", tag = "agents",
    params(("id" = String, Path), ("artifact_id" = String, Path)),
    responses((status = 200, body = String, content_type = "application/octet-stream")))]
pub(crate) async fn download(
    State(service): State<Arc<AgentArtifactService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path((id, artifact_id)): Path<(String, String)>,
) -> Result<Response, ArtifactProblem> {
    let (metadata, bytes) = service
        .read(
            &caller,
            resource.as_ref().map(|resource| &resource.context),
            resource
                .as_ref()
                .and_then(|resource| resource.credential.as_ref())
                .map(DelegatedCredential::value),
            &AgentId(id),
            &artifact_id,
        )
        .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"artifact-{}.bin\"", metadata.sha256),
            ),
        ],
        bytes,
    )
        .into_response())
}

pub(crate) struct ArtifactProblem(ResourceAccessError);
impl From<ResourceAccessError> for ArtifactProblem {
    fn from(error: ResourceAccessError) -> Self {
        Self(error)
    }
}
impl IntoResponse for ArtifactProblem {
    fn into_response(self) -> Response {
        let (status, code, detail) = match self.0 {
            ResourceAccessError::Store(ResourceAccessStoreError::Invalid) => (
                StatusCode::BAD_REQUEST,
                "invalid_artifact_request",
                "Artifact name, media type, content or pagination is invalid.",
            ),
            ResourceAccessError::Store(ResourceAccessStoreError::Capacity) => (
                StatusCode::TOO_MANY_REQUESTS,
                "artifact_capacity",
                "This Agent has reached its published artifact limit.",
            ),
            ResourceAccessError::Store(ResourceAccessStoreError::IdempotencyConflict) => (
                StatusCode::CONFLICT,
                "idempotency_key_reused",
                "The idempotency key was used for different artifact content or metadata.",
            ),
            error => return super::resource_access_problem::ResourceProblem(error).into_response(),
        };
        super::problem_response(status, super::problem_document(status, code, detail, code))
    }
}
