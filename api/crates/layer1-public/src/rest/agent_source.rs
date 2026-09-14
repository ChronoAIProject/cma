//! Owner-scoped creation provenance and published Trigger message shortcuts.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::{
    sandbox::{AgentOrigin, AgentReadService},
    trigger_domain::TriggerService,
    AgentId,
};
use serde::Serialize;
use utoipa::ToSchema;

use super::trigger_openapi_shapes::TriggerQuickMessageBody;
use super::{instance_problem::InstanceProblem, trigger_problem::TriggerProblem};
use crate::AuthenticatedCaller;

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

#[utoipa::path(get, path = "/api/v1/agents/{id}/source", tag = "agents",
    params(("id" = String, Path)),
    responses((status = 200, body = AgentSourceView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(state): State<AgentSourceState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    let agent = match state.reads.read(&caller, &AgentId(id)).await {
        Ok(agent) => agent,
        Err(error) => return InstanceProblem::from(error).into_response(),
    };
    let source = match state.triggers.agent_source(&caller, &agent).await {
        Ok(source) => source,
        Err(error) => return TriggerProblem::from(error).into_response(),
    };
    let origin = match agent.origin {
        AgentOrigin::Manual => AgentOriginView::Manual,
        AgentOrigin::WorkspaceProfile { .. } => AgentOriginView::WorkspaceProfile,
        AgentOrigin::Trigger {
            trigger_id,
            launch_id,
        } => AgentOriginView::Trigger {
            trigger_id,
            launch_id,
        },
    };
    Json(AgentSourceView {
        agent_id: agent.id.to_string(),
        origin,
        trigger: source.map(|source| AgentTriggerSourceView {
            public_id: source.public_id,
            title: source.title,
            revision: source.revision,
            quick_messages: source.quick_messages.into_iter().map(Into::into).collect(),
        }),
    })
    .into_response()
}
