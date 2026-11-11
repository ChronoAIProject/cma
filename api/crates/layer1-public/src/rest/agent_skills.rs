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
    async fn upgrade_supported(&self, _sandbox: &layer2_core::sandbox::Sandbox) -> bool {
        false
    }
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
    /// Effective installed version. An upgrade keeps its previous version until confirmed.
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
        for skill in &agent.work.live_skills {
            let effective = if skill.status == AgentSkillInstallStatus::Installed {
                skill.material.as_ref()
            } else {
                skill.previous.as_ref().or(skill.material.as_ref())
            };
            let row = AgentSkillView {
                id: skill.reference.id.as_str().into(),
                name: effective.and_then(|m| m.name.clone()),
                version: effective
                    .map(|m| m.version.clone())
                    .or_else(|| skill.reference.version.clone())
                    .unwrap_or_default(),
                source: "ornn".into(),
                status: if skill.previous.is_some() {
                    "installed"
                } else {
                    match skill.status {
                        AgentSkillInstallStatus::Pending => "pending",
                        AgentSkillInstallStatus::Installed => "installed",
                        AgentSkillInstallStatus::Failed => "failed",
                    }
                }
                .into(),
                error: skill.error.as_ref().map(|error| match &skill.previous {
                    Some(_) => format!(
                        "Upgrade to version {} failed: {error}",
                        skill.reference.version.as_deref().unwrap_or_default()
                    ),
                    None => error.clone(),
                }),
            };
            if let Some(index) = items.iter().position(|item| item.id == row.id) {
                items[index] = row;
            } else {
                items.push(row);
            }
        }
        let pending = agent
            .work
            .live_skills
            .iter()
            .any(|item| item.status == AgentSkillInstallStatus::Pending)
            || items.iter().any(|item| item.status == "pending");
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
    Ok(Json(
        view(
            lineage.agent,
            &lineage.sandbox,
            state.skill_support.as_deref(),
        )
        .await,
    ))
}
async fn view(
    agent: Agent,
    sandbox: &layer2_core::sandbox::Sandbox,
    support: Option<&dyn AgentSkillRuntimeSupport>,
) -> AgentSkillsView {
    let idle = layer2_core::sandbox::agent_live_skills::idle(&agent);
    let (can_add, can_upgrade) = layer2_core::sandbox::agent_live_skills::capacity(&agent);
    let mut view = AgentSkillsView::from(agent);
    if idle
        && matches!(
            sandbox.status,
            layer2_core::sandbox::SandboxStatus::Active
                | layer2_core::sandbox::SandboxStatus::CredentialRotating
        )
        && sandbox.lifecycle.checkpoint_capture.is_none()
    {
        view.can_install = match support {
            Some(support) => support.supported(sandbox).await,
            None => false,
        };
        view.unavailable_reason = (!view.can_install)
            .then(|| "This Agent runtime does not support installing skills yet.".into());
        if !can_add && !can_upgrade {
            view.can_install = false;
            view.unavailable_reason =
                Some("This Agent has reached its skill installation capacity.".into());
        } else if !can_add
            && !match support {
                Some(support) => support.upgrade_supported(sandbox).await,
                None => false,
            }
        {
            view.can_install = false;
            view.unavailable_reason = Some(
                "This Agent has reached its skill limit and its runtime does not support upgrades."
                    .into(),
            );
        }
    }
    view
}

/// Install an exact skill version, or upgrade an existing skill on a capable idle runtime.
/// Poll GET until pending is false. Refusals are recorded for their Idempotency-Key;
/// after readiness or runtime support changes, submit a new attempt with a new key.
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
                return Err(InstanceProblem::SkillNotReady);
            }
            let reference = SkillRef {
                source: SkillSource::Ornn,
                id: SkillId(body.id),
                version: Some(body.version),
            };
            let previous = layer2_core::sandbox::agent_live_skills::effective(
                &lineage.agent.work,
                &reference.id,
            );
            if let Some(previous) = previous {
                if Some(&previous.version) != reference.version.as_ref()
                    && !match &support {
                        Some(support) => support.upgrade_supported(&lineage.sandbox).await,
                        None => false,
                    }
                {
                    return Err(InstanceProblem::SkillVersionUpgradeUnsupported {
                        id: reference.id.as_str().into(),
                        installed: previous.version.clone(),
                        requested: reference.version.clone().unwrap_or_default(),
                    });
                }
            } else if !match &support {
                Some(support) => support.supported(&lineage.sandbox).await,
                None => false,
            } {
                return Err(InstanceProblem::SkillInstallUnsupported);
            }
            let agent = service
                .install_skill(&operation_caller, &AgentId(id), reference)
                .await
                .map_err(|error| match error {
                    layer2_core::sandbox::AgentOperationError::StatusConflict { .. } => {
                        InstanceProblem::SkillNotReady
                    }
                    other => InstanceProblem::from(other),
                })?;
            Ok((
                StatusCode::ACCEPTED,
                Json(view(agent, &lineage.sandbox, support.as_deref()).await),
            )
                .into_response())
        },
    )
    .await
}
