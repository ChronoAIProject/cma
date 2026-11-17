//! Additive retained partial-output reads; these never project new log events.
use super::{agents::AgentRoutesState, instance_problem::InstanceProblem};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::{sandbox::AgentRetainedOutputPage, AgentId};
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};

/// Discovery only; each chunk still requires fresh conversation authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct RetainedOutputAvailability {
    pub available: bool,
}
impl From<layer2_core::sandbox::AgentRetainedOutputAvailability> for RetainedOutputAvailability {
    fn from(value: layer2_core::sandbox::AgentRetainedOutputAvailability) -> Self {
        Self {
            available: value.available,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedOutputQuery {
    pub cursor: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct RetainedOutputView {
    id: String,
    agent_id: String,
    turn_sequence: u64,
    checkpoint_sequence: u64,
    failure_sequence: u64,
    failed_at: String,
    byte_length: usize,
    /// Always true: this is retained text from a failed turn, never a completed answer.
    partial: bool,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct RetainedOutputPageView {
    output: Option<RetainedOutputView>,
    /// UTF-8 byte offset, independent of event sequence numbers.
    offset: usize,
    /// At most 512 KiB of UTF-8 text. Concatenate successive pages without separators.
    content: String,
    /// Opaque cursor bound to the Agent incarnation and exact checkpoint.
    next: Option<String>,
}

pub(crate) fn response(page: AgentRetainedOutputPage) -> Response {
    let body = RetainedOutputPageView {
        output: page.output.map(|value| RetainedOutputView {
            id: value.id,
            agent_id: value.agent_id.to_string(),
            turn_sequence: value.turn_sequence,
            checkpoint_sequence: value.checkpoint_sequence,
            failure_sequence: value.failure_sequence,
            failed_at: value.failed_at,
            byte_length: value.byte_length,
            partial: true,
        }),
        offset: page.offset,
        content: page.content,
        next: page.next,
    };
    ([(header::CACHE_CONTROL, "private, no-store")], Json(body)).into_response()
}

pub(super) fn routes() -> Router<AgentRoutesState> {
    Router::new().route("/api/v1/agents/{id}/retained-output", get(read))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/retained-output", tag = "agents",
    params(("id" = String, Path), ("cursor" = Option<String>, Query, description = "Omit for discovery and the first chunk; otherwise use next verbatim.")),
    responses((status = 200, body = RetainedOutputPageView), (status = 400, description = "Invalid cursor"), (status = 404, description = "Agent is not visible"), (status = 503, description = "Retained output or its evidence is unavailable")))]
async fn read(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    query: Result<Query<RetainedOutputQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, InstanceProblem> {
    let Query(query) = query.map_err(|_| InstanceProblem::InvalidQuery)?;
    let id = AgentId(id);
    let page = if let Some(resource) = resource {
        let authority = state
            .reads
            .authorize_event_stream(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &id,
            )
            .await?;
        state
            .reads
            .retained_output_for_stream(&authority, None, query.cursor.as_deref())
            .await
    } else {
        state
            .reads
            .retained_output(&caller, &id, query.cursor.as_deref())
            .await
    }
    .map_err(InstanceProblem::from)?;
    Ok(response(page))
}

pub(super) fn document() -> utoipa::openapi::OpenApi {
    #[derive(OpenApi)]
    #[openapi(
        paths(read),
        components(schemas(
            RetainedOutputPageView,
            RetainedOutputView,
            RetainedOutputAvailability
        ))
    )]
    struct Api;
    Api::openapi()
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::sandbox::{AgentRetainedOutput, RETAINED_OUTPUT_CHUNK_BYTES};

    #[tokio::test]
    async fn largest_escaped_chunk_fits_existing_client_response_limit() {
        let content = "\u{1}".repeat(RETAINED_OUTPUT_CHUNK_BYTES);
        let result = response(AgentRetainedOutputPage {
            output: Some(AgentRetainedOutput {
                id: format!("retained_{}", "a".repeat(64)),
                agent_id: AgentId("agt_retained".into()),
                turn_sequence: 3,
                checkpoint_sequence: 5,
                failure_sequence: 6,
                failed_at: "2026-11-17T00:00:00Z".into(),
                byte_length: 1024 * 1024,
            }),
            offset: 0,
            content: content.clone(),
            next: Some(format!("retained_{}:524288", "a".repeat(64))),
        });
        assert_eq!(result.headers()[header::CONTENT_TYPE], "application/json");
        let body = axum::body::to_bytes(result.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        assert!(body.len() > 3 * 1024 * 1024);
        assert!(body.len() < 3 * 1024 * 1024 + 1024);
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(page["content"], content);
        assert_eq!(page["output"]["partial"], true);
    }
}
