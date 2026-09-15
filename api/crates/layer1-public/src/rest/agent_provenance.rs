use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::sandbox::{
    AgentCreationCategory, AgentCreationOrigin, AgentOrigin, AgentProvenanceReading,
    AgentProvenanceService,
};
use serde::Serialize;
use utoipa::ToSchema;

use super::{resource_access_problem::ResourceProblem, resource_access_shapes::ResourceActorView};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};

pub fn agent_provenance_router(service: Arc<AgentProvenanceService>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/provenance", get(read))
        .with_state(service)
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentCreationCategoryView {
    Manual,
    Trigger,
    ChannelBridge,
    LegacyUnknown,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AgentProvenanceView {
    agent_id: String,
    created_at: String,
    creation_origin: AgentCreationCategoryView,
    /// Only a current owner or organization resource manager receives details.
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<AgentProvenanceDetailsView>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AgentProvenanceDetailsView {
    created_by: Option<ResourceActorView>,
    trigger: Option<AgentCreationTriggerView>,
    channel_bridge: Option<AgentCreationBridgeView>,
    workspace_profile_index: Option<u16>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AgentCreationTriggerView {
    trigger_id: String,
    launch_id: String,
    launching_actor: Option<ResourceActorView>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AgentCreationBridgeView {
    bridge_id: String,
    partition_id: String,
    generation: u64,
    source_reference: String,
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/provenance", tag = "agents",
    params(("id" = String, Path)),
    responses((status = 200, body = AgentProvenanceView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(service): State<Arc<AgentProvenanceService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Response {
    let id = layer2_core::AgentId(id);
    let result = match resource {
        Some(resource) => {
            service
                .read(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                )
                .await
        }
        None => service.read_personal(&caller, &id).await,
    };
    match result {
        Ok(value) => Json(AgentProvenanceView::from(value)).into_response(),
        Err(error) => ResourceProblem(error).into_response(),
    }
}

impl From<AgentProvenanceReading> for AgentProvenanceView {
    fn from(value: AgentProvenanceReading) -> Self {
        Self {
            agent_id: value.agent_id.to_string(),
            created_at: value.created_at,
            creation_origin: match value.creation_origin {
                AgentCreationCategory::Manual => AgentCreationCategoryView::Manual,
                AgentCreationCategory::Trigger => AgentCreationCategoryView::Trigger,
                AgentCreationCategory::ChannelBridge => AgentCreationCategoryView::ChannelBridge,
                AgentCreationCategory::LegacyUnknown => AgentCreationCategoryView::LegacyUnknown,
            },
            details: value.details.map(|details| {
                let mut result = AgentProvenanceDetailsView {
                    created_by: details.created_by.map(Into::into),
                    trigger: None,
                    channel_bridge: None,
                    workspace_profile_index: match details.provisioning_origin {
                        AgentOrigin::WorkspaceProfile { index } => Some(index),
                        _ => None,
                    },
                };
                match details.origin {
                    AgentCreationOrigin::Trigger {
                        trigger_id,
                        launch_id,
                        launching_actor,
                    } => {
                        result.trigger = Some(AgentCreationTriggerView {
                            trigger_id,
                            launch_id,
                            launching_actor: launching_actor.map(Into::into),
                        });
                    }
                    AgentCreationOrigin::ChannelBridge {
                        bridge_id,
                        partition_id,
                        generation,
                        source_reference,
                    } => {
                        result.channel_bridge = Some(AgentCreationBridgeView {
                            bridge_id,
                            partition_id,
                            generation,
                            source_reference,
                        });
                    }
                    _ => {}
                }
                result
            }),
        }
    }
}
