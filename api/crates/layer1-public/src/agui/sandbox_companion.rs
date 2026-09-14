use axum::{
    extract::{Path, State},
    response::Response,
    Json,
};
use layer2_core::{
    sandbox::{AgentLineage, SandboxStatus},
    AgentId,
};
use serde_json::{json, Value};

use crate::{conversation::read_failure, AgentConversationState, AuthenticatedCaller};

#[utoipa::path(get, path = "/api/v1/agui/agents/{id}/sandbox", tag = "ag-ui",
    params(("id" = String, Path)), responses((status = 200, body = Value),
    (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn snapshot(
    State(state): State<AgentConversationState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    state
        .reads
        .lineage(&caller, &AgentId(id))
        .await
        .map(|lineage| Json(project(&lineage)))
        .map_err(|error| read_failure(error, "thread_not_found"))
}

pub(super) fn project(lineage: &AgentLineage) -> Value {
    json!({
        "threadId": lineage.agent.id,
        "sandbox": {"id": lineage.sandbox.id, "name": lineage.sandbox.name,
            "status": lineage.sandbox.status,
            "paused": matches!(lineage.sandbox.status, SandboxStatus::Paused | SandboxStatus::Pausing),
            "pausedNotice": lineage.sandbox.lifecycle.paused_notice,
            "rotation": lineage.sandbox.lifecycle.rotation},
        "workspace": {"id": lineage.workspace.id, "name": lineage.workspace.name,
            "path": lineage.workspace.path, "status": lineage.workspace.status},
        "worktree": {"id": lineage.worktree.id, "name": lineage.worktree.name,
            "path": lineage.worktree.path, "status": lineage.worktree.status},
        "firstMessage": lineage.agent.conversation.first_message.as_ref().map(|first| json!({
            "state": first.state, "attempt": first.attempt,
            "queuedAt": first.queued_at, "completedAt": first.completed_at,
            "failedAt": first.failed_at
        }))
    })
}
