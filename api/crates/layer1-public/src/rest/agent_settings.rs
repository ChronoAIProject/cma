//! Per-Agent live settings and durable replay.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    profiles::{PermissionLevel, ThinkingEffort},
    sandbox::{Agent, AgentSettings},
    AgentId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
    agents::{execute_command, AgentRoutesState},
    idempotency::{IdempotencyKey, RequestBody},
    instance_problem::InstanceProblem,
    BodyProblem,
};
use crate::AuthenticatedCaller;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateAgentSettingsRequest {
    pub model: String,
    #[schema(example = "high")]
    pub reasoning_effort: String,
    #[schema(example = "autonomous")]
    pub permission_mode: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentSettingsView {
    pub model: String,
    pub reasoning_effort: String,
    pub permission_mode: String,
    /// A previous update is awaiting runtime confirmation.
    pub pending: bool,
}

impl TryFrom<UpdateAgentSettingsRequest> for AgentSettings {
    type Error = InstanceProblem;
    fn try_from(value: UpdateAgentSettingsRequest) -> Result<Self, Self::Error> {
        let settings = Self {
            model: value.model,
            reasoning_effort: match value.reasoning_effort.as_str() {
                "minimal" => ThinkingEffort::Minimal,
                "low" => ThinkingEffort::Low,
                "medium" => ThinkingEffort::Medium,
                "high" => ThinkingEffort::High,
                "x_high" => ThinkingEffort::XHigh,
                _ => return Err(InstanceProblem::InvalidRequest),
            },
            permission_mode: match value.permission_mode.as_str() {
                "guarded" => PermissionLevel::Guarded,
                "autonomous" => PermissionLevel::Autonomous,
                _ => return Err(InstanceProblem::InvalidRequest),
            },
        };
        settings
            .validate()
            .map_err(|_| InstanceProblem::InvalidRequest)?;
        Ok(settings)
    }
}

impl From<Agent> for AgentSettingsView {
    fn from(agent: Agent) -> Self {
        let settings = agent.settings();
        Self {
            model: settings.model,
            reasoning_effort: match settings.reasoning_effort {
                ThinkingEffort::Minimal => "minimal",
                ThinkingEffort::Low => "low",
                ThinkingEffort::Medium => "medium",
                ThinkingEffort::High => "high",
                ThinkingEffort::XHigh => "x_high",
            }
            .into(),
            permission_mode: match settings.permission_mode {
                PermissionLevel::Guarded => "guarded",
                PermissionLevel::Autonomous => "autonomous",
            }
            .into(),
            pending: agent.settings_pending(),
        }
    }
}

#[utoipa::path(get, operation_id = "read_agent_settings", path = "/api/v1/agents/{id}/settings", tag = "agents",
    params(("id" = String, Path)), responses((status = 200, body = AgentSettingsView),
    (status = 404, body = crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<AgentSettingsView>, InstanceProblem> {
    state
        .reads
        .lineage(&caller, &AgentId(id))
        .await
        .map(|lineage| AgentSettingsView::from(lineage.agent))
        .map(Json)
        .map_err(InstanceProblem::from)
}

#[utoipa::path(patch, operation_id = "update_agent_settings", path = "/api/v1/agents/{id}/settings", tag = "agents",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = UpdateAgentSettingsRequest,
    responses((status = 200, body = AgentSettingsView),
    (status = 409, body = crate::rest::ProblemDocument)))]
pub(crate) async fn update(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<UpdateAgentSettingsRequest>, BodyProblem>,
) -> Response {
    let service = state.agent_operations.clone();
    let operation_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("agent:settings:{id}"),
        body,
        move |body| async move {
            let settings = body.try_into()?;
            let agent = service
                .update_settings(&operation_caller, &AgentId(id), settings)
                .await?;
            Ok(Json(AgentSettingsView::from(agent)).into_response())
        },
    )
    .await
}
