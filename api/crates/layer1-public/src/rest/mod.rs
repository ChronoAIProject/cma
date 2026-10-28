//! REST infrastructure retained while replacement resource routes are rebuilt.

use std::sync::Arc;

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::StoreError;

pub mod admin;
mod bot_setups;
mod bots;
pub use bot_setups::bot_setup_router;
pub use bots::bot_router;
mod agent_access_requests;
pub use agent_access_requests::agent_access_requests_router;
#[cfg(test)]
mod agent_access_requests_tests;
mod agent_artifacts;
mod agent_attachments;
pub use agent_attachments::agent_attachments_router;
mod agent_filesystem;
pub use agent_filesystem::agent_filesystem_router;
mod agent_observation_shapes;
mod agent_openapi_shapes;
pub use agent_artifacts::agent_artifacts_router;
#[cfg(test)]
mod agent_artifacts_tests;
mod agent_channel_usage;
mod agent_command_admission;
mod agent_models;
mod agent_operate;
mod agent_preferences;
mod agent_settings;
mod agent_skills;
mod message_feedback;
pub use agent_channel_usage::agent_channel_usage_router;
pub use agent_skills::AgentSkillRuntimeSupport;
#[cfg(test)]
mod agent_channel_usage_tests;
mod agent_provenance;
mod agent_provenance_inventory;
pub use agent_provenance_inventory::agent_provenance_inventory_router;
#[cfg(test)]
mod agent_provenance_inventory_tests;
mod agent_reads;
mod agent_shared_shapes;
mod agent_source;
pub use agent_provenance::agent_provenance_router;
mod agent_history;
mod agent_lifecycle;
#[cfg(test)]
mod agent_provenance_tests;
mod agent_share_links;
mod agents;
#[cfg(test)]
pub(crate) mod agents_tests;
mod badge;
mod badge_mark;
#[cfg(test)]
mod badge_tests;
mod defaults;
mod desktop;
pub use desktop::desktop_router;
mod documents;
pub use documents::documents_router;
mod embed;
#[cfg(test)]
mod embed_tests;
pub mod openapi;
mod openapi_shapes;
mod organizations;
pub mod pagination;
mod problems;
mod profile_authoring;
mod profiles;
mod profiles_agent;
mod profiles_marketplace;
mod profiles_openapi_shapes;
mod profiles_problem;
mod profiles_references;
mod profiles_sandbox;
#[cfg(test)]
mod profiles_tests;
mod profiles_workspace;
#[cfg(test)]
mod public_contract_tests;
mod remediation;
mod resource_access;
pub(crate) mod resource_access_problem;
mod resource_access_shapes;
#[cfg(test)]
mod resource_access_tests;
#[cfg(test)]
mod shared_agent_tests;
#[cfg(test)]
mod shared_hierarchy_tests;
#[cfg(test)]
mod shared_source_tests;
#[cfg(test)]
mod shared_stream_tests;
pub use resource_access::resource_access_router;
#[cfg(test)]
mod trigger_assets_test_support;

mod bindings;
#[cfg(test)]
mod bindings_tests;
pub use bindings::{
    binding_router, binding_router_with_native_tools, binding_router_with_native_tools_alias,
};
mod gateway_connections;
mod gateway_interactions;
mod gateway_records;
pub use gateway_connections::gateway_connection_record_router;
pub use gateway_interactions::gateway_interaction_router;
#[cfg(test)]
mod gateway_records_tests;
mod hierarchy_shared_shapes;
mod idempotency;
pub(crate) mod instance_problem;
mod instances;
mod instances_openapi_shapes;
pub(crate) mod instances_problem;
#[cfg(test)]
mod instances_tests;
mod launch_events;
mod launch_quota;
mod launches;
mod public_names;
mod sandbox_commands;
#[cfg(test)]
mod sandbox_commands_tests;
mod sandbox_runtime_update;
mod sandbox_service_access;
pub use sandbox_commands::sandbox_commands_router;
mod sandboxes;
mod service_connections;
mod trigger_entry;
mod trigger_openapi_shapes;
mod trigger_placements;
mod trigger_problem;
#[cfg(test)]
mod trigger_routes_tests;
mod triggers;
mod workspaces;
mod worktrees;

pub use agent_history::{AgentHistoryExportError, AgentHistoryExporter};
pub use agent_lifecycle::{
    AgentLifecycleConnectionRequired, AgentLifecycleCreateReceipt, AgentLifecycleCreateRequest,
    AgentLifecycleEventView, AgentLifecycleImageReference, AgentLifecycleOperationReadError,
    AgentLifecycleOperationReader, AgentLifecycleOperationView, AgentLifecyclePermissions,
    AgentLifecycleProfile, AgentLifecycleProvisioner, AgentLifecycleProvisioningError,
    AgentLifecycleRepository, AgentLifecycleSkill,
};
pub use agent_source::agent_source_router;
pub use agents::{agent_router, AgentRoutesState};
pub use embed::{
    EmbedContentSecurityPolicy, EmbedDescriptor, EmbedSnippets, EMBED_V1_JAVASCRIPT_SRI,
};
pub use gateway_records::gateway_record_router;
pub use instances::{hierarchy_router, HierarchyState};
pub use launch_events::{
    FirstMessageStateValue, LaunchBlockedReasonView, LaunchEffectKindValue, LaunchEventKindView,
    LaunchEventView, LaunchFailureView, LaunchStateValue, LaunchStepKeyValue,
    LaunchStepStatusValue, LaunchStepView, RepeatChoiceValue, ServiceRequirementStateValue,
};
pub use launch_quota::{LaunchQuota, LaunchQuotaError, LaunchQuotaPermit, LaunchQuotaRefusal};
pub(crate) use problems::{problem_document, problem_response};
pub use problems::{HttpProblem, Problem, ProblemDocument};
pub use profile_authoring::profile_authoring_router;
pub use profiles::{profiles_router, ProfileRoutePolicy, ProfilesState};
pub use triggers::{trigger_router, TriggerRoutesState};

use crate::{InterfaceProfile, Operation};
use openapi_shapes::ConformanceDocument;

pub(crate) const PROBLEM_BASE: &str = "https://spec.chrono-ai.fun/cma/1.0/problems";
pub const REQUEST_ID_LENGTH: usize = 36;
pub const REQUEST_ID_PATTERN: &str =
    "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

/// Return one canonical caller request identifier from an unambiguous header.
pub fn request_id_from_headers(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all("x-request-id").iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    normalize_request_id(value.to_str().ok()?)
}

/// Accept only canonical lowercase, hyphenated UUID request identifiers.
pub fn normalize_request_id(value: &str) -> Option<&str> {
    let normalized = value.trim_matches(|character: char| character.is_ascii_whitespace());
    if normalized.len() != REQUEST_ID_LENGTH {
        return None;
    }
    let parsed = uuid::Uuid::parse_str(normalized).ok()?;
    (parsed.hyphenated().to_string() == normalized).then_some(normalized)
}

#[derive(Debug)]
pub(crate) struct BodyProblem {
    status: StatusCode,
    detail: String,
    code: &'static str,
}

impl BodyProblem {
    pub(crate) fn missing_json_content_type() -> Self {
        Self {
            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
            detail: "The request body must use an application/json content type.".into(),
            code: "json_content_type_required",
        }
    }
}

impl From<axum::extract::rejection::JsonRejection> for BodyProblem {
    fn from(error: axum::extract::rejection::JsonRejection) -> Self {
        Self {
            status: error.status(),
            detail: error.body_text(),
            code: "invalid_json",
        }
    }
}

impl IntoResponse for BodyProblem {
    fn into_response(self) -> Response {
        let body = problem_document(self.status, self.code, &self.detail, self.code);
        problem_response(self.status, body)
    }
}

pub(crate) enum RequestFailure {
    Body(BodyProblem),
    Store(StoreError),
}

impl From<BodyProblem> for RequestFailure {
    fn from(value: BodyProblem) -> Self {
        Self::Body(value)
    }
}

impl From<StoreError> for RequestFailure {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

impl IntoResponse for RequestFailure {
    fn into_response(self) -> Response {
        match self {
            Self::Body(problem) => problem.into_response(),
            Self::Store(error) => {
                let status = StatusCode::SERVICE_UNAVAILABLE;
                let body =
                    problem_document(status, "store-failed", &error.to_string(), "store_failed");
                problem_response(status, body)
            }
        }
    }
}

/// Profile for the currently composed REST operations.
pub struct RestInterface;

impl InterfaceProfile for RestInterface {
    fn interface_id(&self) -> &'static str {
        "rest"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::Discover,
            Operation::ManageProfiles,
            Operation::ReadProfileMarketplace,
            Operation::ForkMarketplaceProfile,
            Operation::ManageAgents,
            Operation::SendAgentInput,
            Operation::StreamAgentEvents,
            Operation::ResolveAgentApproval,
            Operation::InterruptAgent,
            Operation::SteerAgent,
            Operation::ReadAgentUsage,
            Operation::ReadAgentResources,
            Operation::Administer,
            Operation::ManageSandboxes,
            Operation::ManageWorkspaces,
            Operation::ReadWorktrees,
            Operation::ReadSandboxEvents,
        ]
    }
}

/// State shared by the anonymous badge and owner-only embed descriptor.
#[derive(Clone)]
pub struct TriggerAssetsState {
    pub triggers: Arc<dyn layer2_core::trigger_domain::TriggerStore>,
    public_origin: url::Url,
}

impl TriggerAssetsState {
    /// Validate and retain the deployment's exact public HTTP(S) origin.
    pub fn new(
        triggers: Arc<dyn layer2_core::trigger_domain::TriggerStore>,
        public_origin: &str,
    ) -> Result<Self, InvalidPublicOrigin> {
        let parsed = url::Url::parse(public_origin).map_err(|_| InvalidPublicOrigin)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(InvalidPublicOrigin);
        }
        let canonical = parsed.origin().ascii_serialization();
        let public_origin =
            url::Url::parse(&canonical).expect("an HTTP(S) origin is itself an absolute URL");
        Ok(Self {
            triggers,
            public_origin,
        })
    }

    pub fn public_origin(&self) -> &url::Url {
        &self.public_origin
    }
}

/// The configured Trigger asset origin was not an exact HTTP(S) origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPublicOrigin;

impl std::fmt::Display for InvalidPublicOrigin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "the public Trigger origin must be an HTTP(S) origin without credentials, path, query, or fragment",
        )
    }
}

impl std::error::Error for InvalidPublicOrigin {}

/// Compose the Trigger assets independently of the remaining resource routes.
pub fn trigger_assets_router(state: TriggerAssetsState) -> Router {
    Router::new()
        .route(
            "/api/v1/triggers/public/{public_id}/open/{placement_id}",
            get(trigger_entry::open),
        )
        .route(
            "/api/v1/triggers/public/{public_id}/entry-intent",
            axum::routing::post(trigger_entry::consume),
        )
        .route(
            "/api/v1/triggers/public/{public_id}/badge.svg",
            get(badge::read),
        )
        .route(
            "/api/v1/triggers/{id}/embed",
            get(embed::read).layer(axum::middleware::from_fn(embed::private_headers)),
        )
        .with_state(state)
}

pub(crate) fn is_embed_identifier(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[utoipa::path(
    get,
    path = "/api/v1/conformance",
    tag = "discovery",
    responses((status = 200, description = "Current public CMA state", body = ConformanceDocument))
)]
pub(crate) async fn conformance() -> Json<ConformanceDocument> {
    Json(ConformanceDocument::default())
}

async fn openapi_document() -> Json<utoipa::openapi::OpenApi> {
    Json(openapi::document())
}

async fn agent_manager_openapi_document() -> Json<serde_json::Value> {
    Json(openapi::agent_manager_document())
}

/// Public router for health-independent discovery infrastructure.
pub fn router() -> Router {
    Router::new()
        .route("/api/v1/conformance", get(conformance))
        .route("/openapi.json", get(openapi_document))
        .route(
            "/agent-manager/openapi.json",
            get(agent_manager_openapi_document),
        )
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn discovery_and_openapi_are_both_composed() {
        for path in ["/api/v1/conformance", "/openapi.json"] {
            let response = router()
                .oneshot(axum::http::Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
    }

    #[test]
    fn request_identifiers_are_canonical_and_unambiguous() {
        let canonical = "123e4567-e89b-12d3-a456-426614174000";
        assert_eq!(normalize_request_id(canonical), Some(canonical));
        assert_eq!(
            normalize_request_id(" 123e4567-e89b-12d3-a456-426614174000 "),
            Some(canonical)
        );
        assert!(normalize_request_id("123E4567-E89B-12D3-A456-426614174000").is_none());

        let mut headers = HeaderMap::new();
        headers.append("x-request-id", canonical.parse().unwrap());
        headers.append("x-request-id", canonical.parse().unwrap());
        assert!(request_id_from_headers(&headers).is_none());
    }
}

pub mod event_workers;

pub use agent_models::{
    agent_models_router, AgentModelCatalog, AgentModelCatalogError, AgentModelRuntimeSupport,
    AgentModelView, AgentModelsView,
};
