use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::AgentId;
use serde::Serialize;
use utoipa::ToSchema;

use super::{
    agents::AgentRoutesState, idempotency::IdempotencyKey, instance_problem::InstanceProblem,
};
use crate::AuthenticatedCaller;

#[derive(Serialize, ToSchema)]
pub(crate) struct AgentRestartReceipt {
    agent_id: String,
    worktree_id: String,
    operation_id: String,
    accepted_at: String,
}

/// Reopen the same owner-stopped Agent on its retained Worktree and conversation.
/// Reuse the original Idempotency-Key after an uncertain response.
#[utoipa::path(post, path = "/api/v1/agents/{id}/restart", tag = "agents",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 202, body = AgentRestartReceipt),
        (status = 400, body = crate::rest::ProblemDocument),
        (status = 401, body = crate::rest::ProblemDocument),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument),
        (status = 409, body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)))]
pub(crate) async fn restart(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    let Some(service) = state.owner_agent_restart else {
        return InstanceProblem::AgentNotFound.into_response();
    };
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    match service.restart(&caller, &AgentId(id), &key).await {
        Ok(accepted) => (
            StatusCode::ACCEPTED,
            Json(AgentRestartReceipt {
                agent_id: accepted.agent_id.to_string(),
                worktree_id: accepted.worktree_id.to_string(),
                operation_id: accepted.operation_id,
                accepted_at: accepted.accepted_at,
            }),
        )
            .into_response(),
        Err(error) => InstanceProblem::from_create(error).into_response(),
    }
}
