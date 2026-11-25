use super::{agents::AgentRoutesState, instance_problem::InstanceProblem};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::{sandbox::AgentOriginalRequest, AgentId};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OriginalRequestSource {
    AdmittedFirstTask,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct OriginalRequestView {
    text: String,
    source: OriginalRequestSource,
    turn_sequence: Option<u64>,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum OriginalRequestResponse {
    Ready {
        agent_id: String,
        original_request: OriginalRequestView,
    },
    Unavailable {
        agent_id: String,
        original_request: Option<OriginalRequestView>,
    },
}

pub(super) fn routes() -> Router<AgentRoutesState> {
    Router::new().route("/api/v1/agents/{id}/original-request", get(read))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/original-request", tag = "agents",
    params(("id" = String, Path)),
    responses((status = 200, body = OriginalRequestResponse),
        (status = 401, description = "Authentication required"),
        (status = 403, description = "History permission required"),
        (status = 404, description = "Agent is not owned by the caller"),
        (status = 503, description = "Original request cannot be read safely")))]
pub(super) async fn read(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Response, InstanceProblem> {
    let original = state
        .reads
        .original_request(
            &caller,
            resource.as_ref().map(|r| &r.context),
            resource
                .as_ref()
                .and_then(|r| r.credential.as_ref().map(DelegatedCredential::value)),
            &AgentId(id.clone()),
        )
        .await?;
    Ok(response(id, original))
}

fn response(agent_id: String, original: Option<AgentOriginalRequest>) -> Response {
    let body = match original {
        Some(original) => OriginalRequestResponse::Ready {
            agent_id,
            original_request: OriginalRequestView {
                text: original.text,
                source: OriginalRequestSource::AdmittedFirstTask,
                turn_sequence: original.turn_sequence,
            },
        },
        None => OriginalRequestResponse::Unavailable {
            agent_id,
            original_request: None,
        },
    };
    ([(header::CACHE_CONTROL, "private, no-store")], Json(body)).into_response()
}

#[cfg(test)]
pub(crate) mod tests;
