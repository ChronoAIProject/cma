//! Discover models using the owned Agent's admitted model service.
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::header,
    routing::get,
    Json, Router,
};
use layer2_core::{
    sandbox::{AgentReadService, AgentStatus, Sandbox, SandboxStatus},
    AgentId,
};
use serde::Serialize;
use utoipa::ToSchema;

use super::instance_problem::InstanceProblem;
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AgentModelView {
    pub id: String,
    pub label: String,
    pub available: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentModelsView {
    pub models: Vec<AgentModelView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentModelCatalogError {
    Unavailable,
}

#[async_trait::async_trait]
pub trait AgentModelCatalog: Send + Sync {
    async fn models(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Vec<AgentModelView>, AgentModelCatalogError>;
}

#[async_trait::async_trait]
pub trait AgentModelRuntimeSupport: Send + Sync {
    /// Observe explicit support from this exact bound runtime. Unknown is false.
    async fn supported(&self, sandbox: &Sandbox) -> bool;
}

#[derive(Clone)]
pub(crate) struct AgentModelsState {
    reads: Arc<AgentReadService>,
    catalog: Arc<dyn AgentModelCatalog>,
    support: Arc<dyn AgentModelRuntimeSupport>,
}

pub fn agent_models_router(
    reads: Arc<AgentReadService>,
    catalog: Arc<dyn AgentModelCatalog>,
    support: Arc<dyn AgentModelRuntimeSupport>,
) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/models", get(list))
        .with_state(AgentModelsState {
            reads,
            catalog,
            support,
        })
}

#[utoipa::path(get, operation_id = "list_agent_models", path = "/api/v1/agents/{id}/models", tag = "agents",
    params(("id" = String, Path)), responses((status = 200, body = AgentModelsView),
    (status = 404, body = crate::rest::ProblemDocument), (status = 503, body = crate::rest::ProblemDocument)))]
pub(crate) async fn list(
    State(state): State<AgentModelsState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<AgentModelsView>,
    ),
    InstanceProblem,
> {
    let id = AgentId(id);
    let read = if let Some(resource) = &resource {
        Some(
            state
                .reads
                .configuration_authorized(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                )
                .await?,
        )
    } else {
        None
    };
    let lineage = match &read {
        Some(read) => read.lineage.clone(),
        None => state.reads.lineage(&caller, &id).await?,
    };
    let mut models = state
        .catalog
        .models(&lineage.sandbox)
        .await
        .map_err(|_| InstanceProblem::Unavailable)?;
    let available = matches!(
        lineage.agent.status,
        AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
    ) && matches!(
        lineage.sandbox.status,
        SandboxStatus::Active | SandboxStatus::CredentialRotating
    ) && lineage.sandbox.lifecycle.checkpoint_capture.is_none()
        && state.support.supported(&lineage.sandbox).await;
    for model in &mut models {
        model.available = available;
    }
    if let Some(read) = &read {
        state.reads.validate_configuration_read(read).await?;
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(AgentModelsView { models }),
    ))
}
