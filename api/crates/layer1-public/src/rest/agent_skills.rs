//! Owner-selected additive Agent skills and runtime-confirmed status.
use super::{
    agents::{execute_command, AgentRoutesState},
    idempotency::{IdempotencyKey, RequestBody},
    instance_problem::InstanceProblem,
    BodyProblem,
};
use crate::AuthenticatedCaller;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    profiles::{SkillRef, SkillSource},
    sandbox::{Agent, AgentSkillInstallStatus},
    AgentId, SkillId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[async_trait::async_trait]
pub trait AgentSkillRuntimeSupport: Send + Sync {
    /// Read-only, exact-runtime capability observation. Missing support fails closed.
    async fn supported(&self, sandbox: &layer2_core::sandbox::Sandbox) -> bool;
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstallAgentSkillRequest {
    pub id: String,
    pub version: String,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentSkillView {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub version: String,
    pub source: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentSkillsView {
    pub items: Vec<AgentSkillView>,
    pub pending: bool,
    pub can_install: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}
impl From<Agent> for AgentSkillsView {
    fn from(agent: Agent) -> Self {
        let home_confirmed = agent
            .work
            .receipts
            .iter()
            .any(|r| r.step == layer2_core::sandbox::AgentWorkStep::MaterializeHome);
        let mut items = agent
            .work
            .resolved_skills
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|skill| AgentSkillView {
                id: skill.id.as_str().into(),
                name: skill.name.clone(),
                version: skill.version.clone(),
                source: "ornn".into(),
                status: if home_confirmed {
                    "installed"
                } else {
                    "pending"
                }
                .into(),
                error: None,
            })
            .collect::<Vec<_>>();
        items.extend(agent.work.live_skills.iter().map(|skill| {
            AgentSkillView {
                id: skill.reference.id.as_str().into(),
                name: skill.material.as_ref().and_then(|m| m.name.clone()),
                version: skill
                    .material
                    .as_ref()
                    .map(|m| m.version.clone())
                    .or_else(|| skill.reference.version.clone())
                    .unwrap_or_default(),
                source: "ornn".into(),
                status: match skill.status {
                    AgentSkillInstallStatus::Pending => "pending",
                    AgentSkillInstallStatus::Installed => "installed",
                    AgentSkillInstallStatus::Failed => "failed",
                }
                .into(),
                error: skill.error.clone(),
            }
        }));
        let pending = items.iter().any(|s| s.status == "pending");
        Self {
            items,
            pending,
            can_install: false,
            unavailable_reason: Some(
                "Skills can be installed when this Agent is ready and idle.".into(),
            ),
        }
    }
}
#[utoipa::path(get, operation_id = "read_agent_skills", path = "/api/v1/agents/{id}/skills", tag = "agents",
    params(("id" = String, Path)), responses((status = 200, body = AgentSkillsView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<AgentSkillsView>, InstanceProblem> {
    let lineage = state.reads.lineage(&caller, &AgentId(id)).await?;
    let idle = layer2_core::sandbox::agent_live_skills::idle(&lineage.agent);
    let mut view = AgentSkillsView::from(lineage.agent);
    if idle
        && matches!(
            lineage.sandbox.status,
            layer2_core::sandbox::SandboxStatus::Active
                | layer2_core::sandbox::SandboxStatus::CredentialRotating
        )
        && lineage.sandbox.lifecycle.checkpoint_capture.is_none()
    {
        view.can_install = match &state.skill_support {
            Some(support) => support.supported(&lineage.sandbox).await,
            None => false,
        };
        view.unavailable_reason = (!view.can_install)
            .then(|| "This Agent runtime does not support installing skills yet.".into());
    }
    Ok(Json(view))
}
#[utoipa::path(post, operation_id = "install_agent_skill", path = "/api/v1/agents/{id}/skills", tag = "agents",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)), request_body = InstallAgentSkillRequest,
    responses((status = 202, body = AgentSkillsView), (status = 409, body = crate::rest::ProblemDocument)))]
pub(crate) async fn install(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<InstallAgentSkillRequest>, BodyProblem>,
) -> Response {
    let service = state.agent_operations.clone();
    let operation_caller = caller.clone();
    let reads = state.reads.clone();
    let support = state.skill_support.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("agent:skills:{id}"),
        body,
        move |body| async move {
            let lineage = reads
                .lineage(&operation_caller, &AgentId(id.clone()))
                .await?;
            if !layer2_core::sandbox::agent_live_skills::idle(&lineage.agent) {
                return Err(InstanceProblem::AgentNotReady);
            }
            if !match support {
                Some(support) => support.supported(&lineage.sandbox).await,
                None => false,
            } {
                return Err(InstanceProblem::AgentNotReady);
            }
            let reference = SkillRef {
                source: SkillSource::Ornn,
                id: SkillId(body.id),
                version: Some(body.version),
            };
            let agent = service
                .install_skill(&operation_caller, &AgentId(id), reference)
                .await?;
            Ok((StatusCode::ACCEPTED, Json(AgentSkillsView::from(agent))).into_response())
        },
    )
    .await
}
