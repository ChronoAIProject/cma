use std::collections::BTreeMap;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    sandbox::{Agent, AgentContextOverrides},
    AgentId, AgentProfileId, SandboxId, WorkspaceId,
};
use serde::Deserialize;
use utoipa::ToSchema;

use super::{field_refusal, Conversation};
use crate::{
    conversation::{context_failure, read_failure},
    rest::BodyProblem,
    AgentConversationState, AuthenticatedCaller,
};

#[derive(Default, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub(super) struct CreateConversationRequest {
    metadata: Option<BTreeMap<String, String>>,
}

#[utoipa::path(post, path = "/api/v1/conversations", tag = "responses",
    request_body = CreateConversationRequest,
    responses((status = 200, body = Conversation), (status = 422, body = crate::rest::ProblemDocument)))]
pub(super) async fn create(
    State(state): State<AgentConversationState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<CreateConversationRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => return BodyProblem::from(error).into_response(),
    };
    let mut metadata = body.metadata.unwrap_or_default();
    if metadata.iter().any(|(key, value)| {
        !matches!(key.as_str(), "agent_profile" | "workspace" | "sandbox")
            || value.trim().is_empty()
            || value.len() > 128
            || value.chars().any(char::is_control)
    }) {
        return field_refusal("metadata", "Conversation metadata accepts only owned agent_profile, workspace, and sandbox identifiers.");
    }
    let overrides = AgentContextOverrides {
        agent_profile_id: metadata.remove("agent_profile").map(AgentProfileId),
        workspace_id: metadata.remove("workspace").map(WorkspaceId),
        sandbox_id: metadata.remove("sandbox").map(SandboxId),
    };
    // Own creation once it begins, even when the requesting connection closes.
    match tokio::spawn(
        layer2_core::diagnostic_context::DiagnosticContext::current()
            .scope(async move { state.context.create(&caller, overrides).await }),
    )
    .await
    {
        Ok(Ok(agent)) => Json(project(&agent)).into_response(),
        Ok(Err(error)) => context_failure(error),
        Err(error) => {
            tracing::error!(%error, "conversation context task failed");
            crate::rest::instance_problem::InstanceProblem::Unavailable.into_response()
        }
    }
}

#[utoipa::path(get, path = "/api/v1/conversations/{id}", tag = "responses",
    params(("id" = String, Path)), responses((status = 200, body = Conversation),
    (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn retrieve(
    State(state): State<AgentConversationState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    match state.reads.read(&caller, &AgentId(id)).await {
        Ok(agent) => Json(project(&agent)).into_response(),
        Err(error) => read_failure(error, "conversation_not_found"),
    }
}

fn project(agent: &Agent) -> Conversation {
    let mut metadata = BTreeMap::from([
        ("agent_profile".into(), agent.profile.id.to_string()),
        ("workspace".into(), agent.workspace_id.to_string()),
        ("sandbox".into(), agent.sandbox_id.to_string()),
    ]);
    if let Some(title) = &agent.title {
        metadata.insert("title".into(), title.clone());
    }
    Conversation {
        id: agent.id.to_string(),
        object: "conversation",
        created_at: layer2_core::seconds_from_rfc3339(&agent.created_at).unwrap_or_default(),
        metadata,
    }
}
