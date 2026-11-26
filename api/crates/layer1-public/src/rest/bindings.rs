//! External binding management projects the core reservation and lifecycle.
use crate::AuthenticatedCaller;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::put,
    Json, Router,
};
use layer2_core::sandbox::binding::{
    BindingConversation, BindingConversationState, BindingError, BindingService, BindingState,
    ProvidedCredential, ProviderBinding,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
pub(super) mod inspection;
pub(super) mod reset;
pub use reset::binding_conversation_reset_router;
mod native_endpoints;
use native_endpoints::validate_native_endpoint;
pub(super) mod preparation;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PutBindingRequest {
    pub schema_version: u16,
    #[serde(default)]
    pub target: BindingTarget,
    pub profile: serde_json::Value,
    pub profile_digest: String,
    pub agent_key: String,
    pub tools: BindingTools,
}
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BindingTarget {
    Profile {},
    ExistingAgent { agent_id: String },
}
impl Default for BindingTarget {
    fn default() -> Self {
        Self::Profile {}
    }
}
impl BindingTarget {
    fn is_profile(&self) -> bool {
        matches!(self, Self::Profile {})
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BindingTools {
    pub native_mcp_url: String,
    pub nyxid_service_slug: String,
}

#[derive(Serialize, ToSchema)]
pub struct BindingRefusal {
    pub field: String,
    pub reason: String,
}
#[derive(Serialize, ToSchema)]
pub struct BindingView {
    /// Maximum queue wait for newly admitted binding messages, in seconds.
    #[schema(required = false)]
    pub queue_lifetime_seconds: u32,
    pub binding_id: String,
    #[serde(skip_serializing_if = "BindingTarget::is_profile")]
    pub target: BindingTarget,
    pub owner: String,
    pub profile_digest: String,
    pub sandbox_id: String,
    pub workspace_id: String,
    pub level: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub honoured: Vec<String>,
    pub refused: Vec<BindingRefusal>,
    pub generation: u64,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Serialize, ToSchema)]
pub struct BindingConversationView {
    pub conversation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub state: String,
}

#[derive(Clone)]
pub(super) struct BindingRouteState {
    service: Arc<BindingService>,
    native_endpoint: Option<String>,
    native_advertisement_aliases: Vec<String>,
    preparation_enabled: bool,
    reset_enabled: bool,
}

pub fn binding_router(service: Arc<BindingService>) -> Router {
    routes(BindingRouteState {
        service,
        native_endpoint: None,
        native_advertisement_aliases: vec![],
        preparation_enabled: false,
        reset_enabled: false,
    })
}

/// The configured destination is trusted deployment authority, never request data.
/// Agent startup additionally requires the actual runtime's proxy support proof.
pub fn binding_router_with_native_tools(
    service: Arc<BindingService>,
    endpoint: &str,
) -> Result<Router, String> {
    binding_router_with_native_tools_alias(service, endpoint, endpoint)
}

/// Accept one exact deployment-owned advertisement alias while retaining the
/// configured destination as immutable binding and runtime proxy authority.
pub fn binding_router_with_native_tools_alias(
    service: Arc<BindingService>,
    endpoint: &str,
    advertisement_alias: &str,
) -> Result<Router, String> {
    binding_router_with_native_tools_alias_and_preparation(
        service,
        endpoint,
        advertisement_alias,
        false,
    )
}

pub fn binding_router_with_native_tools_alias_and_preparation(
    service: Arc<BindingService>,
    endpoint: &str,
    advertisement_alias: &str,
    preparation_enabled: bool,
) -> Result<Router, String> {
    binding_router_with_native_tools_aliases_and_preparation(
        service,
        endpoint,
        &[advertisement_alias.into()],
        preparation_enabled,
    )
}

/// Retain exact deployment-owned advertisements without changing runtime authority.
pub fn binding_router_with_native_tools_aliases_and_preparation(
    service: Arc<BindingService>,
    endpoint: &str,
    advertisement_aliases: &[String],
    preparation_enabled: bool,
) -> Result<Router, String> {
    validate_native_endpoint(endpoint)?;
    for alias in advertisement_aliases {
        validate_native_endpoint(alias)?;
    }
    Ok(routes(BindingRouteState {
        service,
        native_endpoint: Some(endpoint.into()),
        native_advertisement_aliases: advertisement_aliases.to_vec(),
        preparation_enabled,
        reset_enabled: false,
    }))
}

fn routes(state: BindingRouteState) -> Router {
    Router::new()
        .merge(preparation::routes())
        .route(
            "/api/v1/cmaeg/bindings/{binding_id}",
            put(put_binding).get(get_binding).delete(delete_binding),
        )
        .route(
            "/api/v1/cmaeg/bindings/{binding_id}/conversations",
            axum::routing::get(inspection::catalog),
        )
        .route(
            "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}",
            put(put_conversation)
                .delete(delete_conversation)
                .get(inspection::get),
        )
        .route(
            "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/profile-drift",
            axum::routing::post(inspection::desired_drift),
        )
        .with_state(state)
        .layer(axum::middleware::from_fn(no_store))
}
async fn no_store(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[utoipa::path(put, path = "/api/v1/cmaeg/bindings/{binding_id}", tag = "provider bindings",
    params(("binding_id" = String, Path)), request_body = PutBindingRequest,
    responses((status = 200, body = BindingView), (status = 400), (status = 401), (status = 403), (status = 409), (status = 503)))]
async fn put_binding(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    body: Result<Json<PutBindingRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(error) => return super::BodyProblem::from(error).into_response(),
    };
    let credential = match ProvidedCredential::new(body.agent_key) {
        Ok(value) => value,
        Err(_) => return error(BindingError::InvalidRequest("invalid agent key".into())),
    };
    let endpoint = url::Url::parse(&body.tools.native_mcp_url);
    if body.schema_version != 1
        || body.tools.nyxid_service_slug != "cmaeg"
        || !endpoint.as_ref().is_ok_and(|url| {
            matches!(url.scheme(), "https" | "http")
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
        })
    {
        return error(BindingError::InvalidRequest(
            "invalid binding schema or tools advertisement".into(),
        ));
    }
    if state.native_endpoint.as_ref().is_some_and(|expected| {
        expected != &body.tools.native_mcp_url
            && !state
                .native_advertisement_aliases
                .contains(&body.tools.native_mcp_url)
    }) {
        return error(BindingError::InvalidRequest(
            "tools advertisement differs from the configured gateway".into(),
        ));
    }
    let result = if let BindingTarget::ExistingAgent { agent_id } = body.target {
        state
            .service
            .put_existing_agent(
                &caller,
                &id,
                body.profile,
                &body.profile_digest,
                &credential,
                layer2_core::AgentId(agent_id),
            )
            .await
    } else {
        state
            .service
            .put_with_tool_endpoint(
                &caller,
                &id,
                body.profile,
                &body.profile_digest,
                &credential,
                state.native_endpoint,
            )
            .await
    };
    match result {
        Ok(value) => Json(BindingView::from(value)).into_response(),
        Err(value) => error(value),
    }
}

#[utoipa::path(get, path = "/api/v1/cmaeg/bindings/{binding_id}", tag = "provider bindings",
    params(("binding_id" = String, Path)), responses((status = 200, body = BindingView), (status = 401), (status = 403), (status = 404), (status = 409), (status = 503)))]
async fn get_binding(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    match state.service.read(&caller, &id).await {
        Ok(value) => Json(BindingView::from(value)).into_response(),
        Err(value) => error(value),
    }
}
#[utoipa::path(delete, path = "/api/v1/cmaeg/bindings/{binding_id}", tag = "provider bindings",
    params(("binding_id" = String, Path)), responses((status = 204), (status = 401), (status = 403), (status = 409), (status = 503)))]
async fn delete_binding(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    match state.service.retire(&caller, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(value) => error(value),
    }
}
#[utoipa::path(put, path = "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}", tag = "provider bindings",
    params(("binding_id" = String, Path), ("conversation_id" = String, Path), ("x-cma-binding-generation" = Option<u64>, Header)),
    responses((status = 201, body = BindingConversationView), (status = 200, body = BindingConversationView), (status = 400), (status = 401), (status = 403), (status = 404), (status = 409), (status = 503)))]
async fn put_conversation(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, conversation)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let generation = match preparation::generation_header(&headers) {
        Ok(value) => value,
        Err(value) => return error(value),
    };
    match state.service.read(&caller, &id).await {
        Ok(binding)
            if binding.existing_agent.is_none()
                && binding.profile.session_affinity
                    == layer2_core::sandbox::binding::SessionAffinity::SourceUser =>
        {
            if binding.state != BindingState::Ready
                || binding.require_generation(generation).is_err()
            {
                return error(BindingError::Conflict);
            }
            if !valid_conversation(&conversation) {
                return error(BindingError::InvalidRequest(
                    "invalid conversation identifier".into(),
                ));
            }
            // Actor affinity can only resolve after an authenticated event arrives.
            // A transport conversation is acknowledged without reserving an Agent.
            return Json(BindingConversationView {
                conversation_id: conversation,
                agent_id: None,
                state: "ready".into(),
            })
            .into_response();
        }
        Ok(_) => {}
        Err(value) => return error(value),
    }
    match state
        .service
        .ensure_conversation_at_generation(&caller, &id, &conversation, generation)
        .await
    {
        Ok((value, created)) => {
            let mut view = BindingConversationView::from(value);
            if view.conversation_id.starts_with("inc_") {
                view.conversation_id = conversation;
            }
            (
                if created {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                Json(view),
            )
                .into_response()
        }
        Err(value) => error(value),
    }
}
#[utoipa::path(delete, path = "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}", tag = "provider bindings",
    params(("binding_id" = String, Path), ("conversation_id" = String, Path)),
    responses((status = 204), (status = 400), (status = 401), (status = 403), (status = 404), (status = 409), (status = 503)))]
async fn delete_conversation(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, conversation)): Path<(String, String)>,
) -> Response {
    match state.service.read_for_retirement(&caller, &id).await {
        Ok(binding)
            if binding.existing_agent.is_none()
                && binding.profile.session_affinity
                    == layer2_core::sandbox::binding::SessionAffinity::SourceUser =>
        {
            if !valid_conversation(&conversation) {
                return error(BindingError::InvalidRequest(
                    "invalid conversation identifier".into(),
                ));
            }
            // A transport chat can contain several users, and a user can use
            // several chats. Removing that transport association cannot retire
            // provider-owned user sessions. Binding retirement cleans them up.
            return StatusCode::NO_CONTENT.into_response();
        }
        Ok(_) => {}
        Err(value) => return error(value),
    }
    match state
        .service
        .retire_conversation(&caller, &id, &conversation)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(value) => error(value),
    }
}

fn valid_conversation(value: &str) -> bool {
    (8..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl From<ProviderBinding> for BindingView {
    fn from(value: ProviderBinding) -> Self {
        let (state, reason) = match value.state {
            BindingState::Provisioning => ("provisioning", None),
            BindingState::Ready => ("ready", None),
            BindingState::Retired => ("retired", None),
            BindingState::Failed { reason } => ("failed", Some(reason)),
        };
        Self {
            queue_lifetime_seconds: value.profile.effective_queue_lifetime_seconds(),
            binding_id: value.binding_id,
            target: value
                .existing_agent
                .map_or(BindingTarget::Profile {}, |agent| {
                    BindingTarget::ExistingAgent {
                        agent_id: agent.agent_id.to_string(),
                    }
                }),
            owner: value.owner.to_string(),
            profile_digest: value.profile_digest,
            sandbox_id: value.sandbox_id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            level: format!("l{}", value.level),
            state: state.into(),
            reason,
            honoured: value.honoured,
            refused: value
                .refused
                .into_iter()
                .map(|value| BindingRefusal {
                    field: value.field,
                    reason: value.reason,
                })
                .collect(),
            generation: value.generation,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}
impl From<BindingConversation> for BindingConversationView {
    fn from(value: BindingConversation) -> Self {
        let state = match value.state {
            BindingConversationState::Provisioning => "provisioning",
            BindingConversationState::Ready => "ready",
            BindingConversationState::Failed => "failed",
            BindingConversationState::Retiring => "retiring",
            BindingConversationState::Retired => "retired",
        };
        Self {
            conversation_id: value.conversation_id,
            agent_id: Some(value.agent_id.to_string()),
            state: state.into(),
        }
    }
}
fn error(error: BindingError) -> Response {
    let (status, code) = match &error {
        BindingError::ResetRefused(code) => (StatusCode::CONFLICT, *code),
        BindingError::AdmissionPaused => return super::admission_problem::paused(),
        BindingError::GlobalCapacityReached => return super::admission_problem::home_capacity(),
        BindingError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
        BindingError::NotFound => (StatusCode::NOT_FOUND, "binding_not_found"),
        BindingError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "invalid_binding"),
        BindingError::Conflict => (StatusCode::CONFLICT, "binding_not_ready"),
        BindingError::SkillVersionUpdateUnsupported { .. } => (
            StatusCode::CONFLICT,
            "binding_skill_version_update_unsupported",
        ),
        BindingError::ReconfigurationUnsupported => {
            (StatusCode::CONFLICT, "binding_reconfiguration_unsupported")
        }
        BindingError::ResourcesUnavailable => {
            (StatusCode::CONFLICT, "binding_resources_unavailable")
        }
        BindingError::Unavailable(_) | BindingError::Corrupt(_) => {
            (StatusCode::SERVICE_UNAVAILABLE, "binding_unavailable")
        }
    };
    let detail = if status == StatusCode::SERVICE_UNAVAILABLE {
        "Binding service is unavailable.".into()
    } else {
        error.to_string()
    };
    super::problem_response(status, super::problem_document(status, code, &detail, code))
}

#[cfg(test)]
mod error_tests;
