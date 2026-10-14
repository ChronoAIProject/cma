//! Current-authority creation provenance and personal Trigger message shortcuts.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::{
    sandbox::{
        AgentOrigin, AgentReadService, AgentSourceReadError, AgentSourceReading, SharedAgentOrigin,
    },
    trigger_domain::TriggerService,
    AgentId,
};
use serde::Serialize;
use utoipa::ToSchema;

use super::trigger_openapi_shapes::TriggerQuickMessageBody;
use super::{instance_problem::InstanceProblem, trigger_problem::TriggerProblem};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};

#[derive(Clone)]
pub(crate) struct AgentSourceState {
    reads: Arc<AgentReadService>,
    triggers: Arc<TriggerService>,
}

pub fn agent_source_router(reads: Arc<AgentReadService>, triggers: Arc<TriggerService>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/source", get(read))
        .with_state(AgentSourceState { reads, triggers })
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AgentOriginView {
    Manual,
    WorkspaceProfile,
    Trigger {
        trigger_id: String,
        launch_id: String,
    },
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentSourceView {
    agent_id: String,
    origin: AgentOriginView,
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger: Option<AgentTriggerSourceView>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentTriggerSourceView {
    public_id: String,
    title: String,
    revision: u32,
    quick_messages: Vec<TriggerQuickMessageBody>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum AccessibleAgentSourceView {
    Personal(AgentSourceView),
    Shared(SharedAgentSourceView),
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum SharedSourceAccess {
    Shared,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SharedAgentSourceView {
    access: SharedSourceAccess,
    agent_id: String,
    origin: SharedAgentOriginView,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SharedAgentOriginView {
    Manual,
    WorkspaceProfile,
    Trigger,
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/source", tag = "agents",
    params(("id" = String, Path)),
    responses((status = 200, body = AccessibleAgentSourceView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(state): State<AgentSourceState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Response {
    let id = AgentId(id);
    let result = match resource {
        Some(resource) => {
            state
                .reads
                .source_authorized(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                    &state.triggers,
                )
                .await
        }
        None => state.reads.source(&caller, &id, &state.triggers).await,
    };
    match result {
        Ok(source) => Json(AccessibleAgentSourceView::from(source)).into_response(),
        Err(AgentSourceReadError::Agent(error)) => InstanceProblem::from(error).into_response(),
        Err(AgentSourceReadError::Trigger(error)) => TriggerProblem::from(error).into_response(),
    }
}

impl From<AgentSourceReading> for AccessibleAgentSourceView {
    fn from(value: AgentSourceReading) -> Self {
        let (agent_id, original, source) = match value {
            AgentSourceReading::Personal {
                agent_id,
                origin,
                trigger,
            } => (agent_id, origin, trigger),
            AgentSourceReading::Shared { agent_id, origin } => {
                return Self::Shared(SharedAgentSourceView {
                    access: SharedSourceAccess::Shared,
                    agent_id: agent_id.to_string(),
                    origin: match origin {
                        SharedAgentOrigin::Manual => SharedAgentOriginView::Manual,
                        SharedAgentOrigin::WorkspaceProfile => {
                            SharedAgentOriginView::WorkspaceProfile
                        }
                        SharedAgentOrigin::Trigger => SharedAgentOriginView::Trigger,
                    },
                })
            }
        };
        let origin = match original {
            AgentOrigin::Manual | AgentOrigin::Lifecycle { .. } => AgentOriginView::Manual,
            AgentOrigin::WorkspaceProfile { .. } => AgentOriginView::WorkspaceProfile,
            AgentOrigin::Trigger {
                trigger_id,
                launch_id,
            } => AgentOriginView::Trigger {
                trigger_id,
                launch_id,
            },
        };
        Self::Personal(AgentSourceView {
            agent_id: agent_id.to_string(),
            origin,
            trigger: source.map(|source| AgentTriggerSourceView {
                public_id: source.public_id,
                title: source.title,
                revision: source.revision,
                quick_messages: source.quick_messages.into_iter().map(Into::into).collect(),
            }),
        })
    }
}
