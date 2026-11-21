//! Explicit recovery of a never-ready Agent's failed initial worktree preparation.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{AgentId, Capability};

use super::{
    agent_openapi_shapes::AgentView,
    agents::AgentRoutesState,
    idempotency::{self, operation_scope, IdempotencyKey},
    instance_problem::InstanceProblem,
};
use crate::AuthenticatedCaller;

/// Retry initial worktree preparation on the same Agent and persistent volume.
/// Only a failed Agent that never became ready is eligible. The failed worktree
/// and its evidence are retained; a fresh worktree is allocated for this attempt.
/// Reuse the original Idempotency-Key when recovering an uncertain response.
#[utoipa::path(post, path = "/api/v1/agents/{id}/recover-worktree", tag = "agents",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 202, body = AgentView), (status = 409, body = crate::rest::ProblemDocument),
        (status = 403, body = crate::rest::ProblemDocument), (status = 404, body = crate::rest::ProblemDocument)))]
pub(crate) async fn recover_worktree(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    if !caller.authorization.allows(Capability::UseOwnSandboxes)
        && !caller.authorization.allows(Capability::ControlAnySandbox)
    {
        return InstanceProblem::Forbidden.into_response();
    }
    let key = match key.required() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let scope = operation_scope("agent:recover-worktree", &[&id]);
    let operation = format!(
        "awr_{}",
        key.confirmation_hash(&format!("{scope}:{}", caller.subject))
            .expect("required operation key")
    );
    let guard = match idempotency::required_empty_command(
        key,
        state.operation_records.clone(),
        &caller,
        scope,
    )
    .await
    {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let result = async {
        let agent = state
            .agents
            .recover_worktree(&caller, &AgentId(id), &operation)
            .await
            .map_err(InstanceProblem::from_create)?;
        let lineage = state.reads.recovery_lineage(&caller, &agent.id).await?;
        let view = AgentView::from_lineage(lineage, None);
        Ok::<_, InstanceProblem>((StatusCode::ACCEPTED, Json(view)).into_response())
    }
    .await;
    guard
        .finish(result.map_err(IntoResponse::into_response))
        .await
}
