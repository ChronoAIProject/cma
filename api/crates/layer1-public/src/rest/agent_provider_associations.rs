//! Retained provider associations are membership facts, not event-use history.
use super::resource_access_problem::ResourceProblem;
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::{
    resource_access::{ResourceAccessError, ResourceAccessStoreError},
    sandbox::{provider_associations::*, store::PageRequest},
    AgentId,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

pub fn agent_provider_associations_router(service: Arc<AgentProviderAssociationService>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/provider-associations", get(read))
        .layer(axum::middleware::from_fn(private_headers))
        .with_state(service)
}
async fn private_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Params {
    limit: Option<u16>,
    cursor: Option<String>,
}
/// Read retained provider membership with current Agent resource management authority.
/// No lifecycle operation, runtime call or usage-history backfill occurs. Retired
/// associations remain visible. Timestamps describe the reservation, not event use.
#[utoipa::path(get, path="/api/v1/agents/{id}/provider-associations", tag="agents",
    params(("id"=String, Path), ("limit"=Option<u16>, Query, minimum=1, maximum=200), ("cursor"=Option<String>, Query)),
    responses((status=200, body=AgentProviderAssociationsView), (status=400, body=crate::rest::ProblemDocument), (status=401, body=crate::rest::ProblemDocument), (status=403, body=crate::rest::ProblemDocument), (status=404, body=crate::rest::ProblemDocument), (status=409, body=crate::rest::ProblemDocument), (status=503, body=crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(service): State<Arc<AgentProviderAssociationService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    params: Result<Query<Params>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let page = match params {
        Ok(Query(params)) => PageRequest::new(params.limit.unwrap_or(50), params.cursor),
        Err(_) => return invalid_query(),
    };
    let page = match page {
        Ok(page) => page,
        Err(_) => return invalid_query(),
    };
    let id = AgentId(id);
    let result = match resource {
        Some(resource) => {
            service
                .read(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                    &page,
                )
                .await
        }
        None => service.read_personal(&caller, &id, &page).await,
    };
    match result {
        Ok(value) => Json(AgentProviderAssociationsView::from(value)).into_response(),
        Err(ResourceAccessError::Store(ResourceAccessStoreError::Invalid)) => invalid_query(),
        Err(error) => ResourceProblem(error).into_response(),
    }
}
fn invalid_query() -> Response {
    super::problem_response(
        StatusCode::BAD_REQUEST,
        super::problem_document(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "Provider associations require a limit from 1 through 200 and a continuation from the same Agent incarnation.",
            "invalid_query",
        ),
    )
}
#[derive(Serialize, ToSchema)]
pub(crate) struct AgentProviderAssociationsView {
    agent_id: String,
    owner: String,
    agent_created_at: String,
    items: Vec<ProviderAssociationView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<String>,
}
impl From<AgentProviderAssociationReading> for AgentProviderAssociationsView {
    fn from(value: AgentProviderAssociationReading) -> Self {
        Self {
            agent_id: value.agent_id,
            owner: value.owner,
            agent_created_at: value.agent_created_at,
            items: value.items.into_iter().map(Into::into).collect(),
            next: value.next,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub(crate) struct ProviderAssociationView {
    binding_id: String,
    binding_generation: u64,
    provider_conversation_id: String,
    relationship: ProviderAssociationRelationshipView,
    sandbox_id: String,
    workspace_id: String,
    worktree_id: String,
    conversation_state: ProviderAssociationConversationStateView,
    binding_state: ProviderAssociationBindingStateView,
    current_binding_generation: u64,
    is_current_generation: bool,
    association_created_at: String,
    association_updated_at: String,
}
impl From<ProviderAssociation> for ProviderAssociationView {
    fn from(value: ProviderAssociation) -> Self {
        Self {
            binding_id: value.binding_id,
            binding_generation: value.binding_generation,
            provider_conversation_id: value.provider_conversation_id,
            relationship: value.relationship.into(),
            sandbox_id: value.sandbox_id,
            workspace_id: value.workspace_id,
            worktree_id: value.worktree_id,
            conversation_state: value.conversation_state.into(),
            binding_state: value.binding_state.into(),
            current_binding_generation: value.current_binding_generation,
            is_current_generation: value.is_current_generation,
            association_created_at: value.association_created_at,
            association_updated_at: value.association_updated_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderAssociationRelationshipView {
    Managed,
    Borrowed,
}
impl From<ProviderAssociationRelationship> for ProviderAssociationRelationshipView {
    fn from(value: ProviderAssociationRelationship) -> Self {
        match value {
            ProviderAssociationRelationship::Managed => Self::Managed,
            ProviderAssociationRelationship::Borrowed => Self::Borrowed,
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderAssociationConversationStateView {
    Provisioning,
    Ready,
    Failed,
    Retiring,
    Retired,
}
impl From<layer2_core::sandbox::binding::BindingConversationState>
    for ProviderAssociationConversationStateView
{
    fn from(value: layer2_core::sandbox::binding::BindingConversationState) -> Self {
        match value {
            layer2_core::sandbox::binding::BindingConversationState::Provisioning => {
                Self::Provisioning
            }
            layer2_core::sandbox::binding::BindingConversationState::Ready => Self::Ready,
            layer2_core::sandbox::binding::BindingConversationState::Failed => Self::Failed,
            layer2_core::sandbox::binding::BindingConversationState::Retiring => Self::Retiring,
            layer2_core::sandbox::binding::BindingConversationState::Retired => Self::Retired,
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderAssociationBindingStateView {
    Provisioning,
    Ready,
    Failed,
    Retired,
}
impl From<ProviderAssociationBindingState> for ProviderAssociationBindingStateView {
    fn from(value: ProviderAssociationBindingState) -> Self {
        match value {
            ProviderAssociationBindingState::Provisioning => Self::Provisioning,
            ProviderAssociationBindingState::Ready => Self::Ready,
            ProviderAssociationBindingState::Failed => Self::Failed,
            ProviderAssociationBindingState::Retired => Self::Retired,
        }
    }
}
