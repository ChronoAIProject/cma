//! REST infrastructure retained while replacement resource routes are rebuilt.

use std::sync::Arc;

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::StoreError;

mod account_deletion;
pub mod admin;
pub(crate) mod admission_problem;
pub use account_deletion::{account_deletion_problem, account_deletion_router};
#[cfg(test)]
mod account_deletion_tests;
mod bot_setups;
mod bots;
pub mod legal_consent;
pub mod privacy_export;
pub use bot_setups::bot_setup_router;
pub use bots::bot_router;
mod agent_access_requests;
mod agent_capacity_recovery;
pub use agent_access_requests::agent_access_requests_router;
pub use agent_capacity_recovery::{
    agent_capacity_recovery_commands_router, agent_capacity_recovery_router,
};
#[cfg(test)]
mod agent_access_requests_tests;
mod agent_artifacts;
mod answer_deliverables;
#[cfg(test)]
mod answer_deliverables_tests;
pub use answer_deliverables::answer_deliverables_router;
mod agent_attachments;
pub use agent_attachments::agent_attachments_router;
mod agent_filesystem;
pub mod binding_media;
pub use agent_filesystem::agent_filesystem_router;
mod agent_list_shapes;
mod agent_observation_shapes;
mod agent_openapi_shapes;
pub(crate) mod task_acceptance_shapes;
pub use agent_artifacts::agent_artifacts_router;
#[cfg(test)]
mod agent_artifacts_tests;
mod agent_channel_usage;
pub(crate) mod sandbox_metrics;
mod sandbox_usage;
pub use sandbox_metrics::{sandbox_metrics_router, UsageReportViewDisabled};
pub use sandbox_usage::sandbox_usage_router;
mod agent_provider_associations;
pub use agent_provider_associations::agent_provider_associations_router;
mod agent_command_admission;
mod agent_models;
mod agent_operate;
mod agent_preferences;
#[cfg(test)]
mod agent_provider_associations_tests;
mod agent_settings;
mod agent_skills;
mod agent_worktree_recovery;
mod dependency_diagnostics;
mod message_feedback;
mod message_feedback_reasons;
mod owner_agent_restart;
mod suggestion_shapes;
mod suggestions;
pub use agent_channel_usage::agent_channel_usage_router;
pub use agent_skills::AgentSkillRuntimeSupport;
#[cfg(test)]
mod agent_channel_usage_tests;
mod agent_provenance;
mod agent_provenance_inventory;
pub use agent_provenance_inventory::agent_provenance_inventory_router;
mod agent_original_request;
#[cfg(test)]
mod agent_provenance_inventory_tests;
mod agent_reads;
mod agent_reply_preview;
pub(crate) mod agent_retained_output;
mod agent_shared_shapes;
mod agent_source;
pub use agent_provenance::agent_provenance_router;
mod agent_history;
mod agent_lifecycle;
mod desktop_creation;
pub mod retained_runtime_transition;
pub mod runtime_canary;
pub use desktop_creation::DesktopCreationScope;
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
pub use desktop::{
    desktop_router, desktop_router_with_origins, permitted_origin as desktop_origin_permitted,
};
mod documents;
pub use documents::documents_router;
mod embed;
#[cfg(test)]
mod embed_tests;
#[cfg(test)]
mod history_export_tests;
pub mod openapi;
pub mod openapi_cache;
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
mod resource_revocations;
#[cfg(test)]
mod resource_revocations_tests;
pub use resource_revocations::resource_access_router_with_revocations;
#[cfg(test)]
mod resource_access_tests;
#[cfg(test)]
mod shared_agent_tests;
#[cfg(test)]
mod shared_hierarchy_tests;
#[cfg(test)]
mod shared_source_tests;
#[cfg(test)]
pub(crate) mod shared_stream_tests;
pub use resource_access::resource_access_router;
mod browser_telemetry;
#[cfg(test)]
mod trigger_assets_test_support;
pub use browser_telemetry::{browser_telemetry_router, BrowserFault};
mod bindings;
#[cfg(test)]
mod bindings_tests;
pub mod worker_credentials;
pub use bindings::{
    binding_conversation_reset_router, binding_router, binding_router_with_native_tools,
    binding_router_with_native_tools_alias, binding_router_with_native_tools_alias_and_preparation,
    binding_router_with_native_tools_aliases_and_preparation,
};
mod gateway_connections;
mod gateway_interactions;
mod gateway_records;
pub use gateway_connections::gateway_connection_record_router;
pub use gateway_interactions::gateway_interaction_router;
pub mod creation_outcomes;
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
mod launch_subject;
mod launches;
mod public_names;
mod sandbox_always_on;
mod sandbox_colors;
mod sandbox_commands;
#[cfg(test)]
mod sandbox_commands_tests;
mod sandbox_retained_recovery;
mod sandbox_runtime_update;
mod sandbox_service_access;
pub use sandbox_commands::sandbox_commands_router;
mod sandbox_capacity;
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
use crate::{InterfaceProfile, Operation};
pub use agent_history::{AgentHistoryExportError, AgentHistoryExporter};
pub use agent_lifecycle::{
    AgentLifecycleConnectionRequired, AgentLifecycleCreateReceipt, AgentLifecycleCreateRequest,
    AgentLifecycleEventView, AgentLifecycleFailure, AgentLifecycleImageReference,
    AgentLifecycleOperationReadError, AgentLifecycleOperationReader, AgentLifecycleOperationView,
    AgentLifecyclePermissions, AgentLifecycleProfile, AgentLifecycleProvisioner,
    AgentLifecycleProvisioningError, AgentLifecycleRepository, AgentLifecycleSkill,
    AgentLifecycleTitleMode, GitHubRepositoryNeed, RepositoryAccessRefusal, RepositoryScopeRepair,
};
pub use agent_source::agent_source_router;
pub use agents::{
    agent_router, personal_agent_inventory_router, AgentRoutesState, PersonalAgentInventory,
};
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
use openapi_shapes::ConformanceDocument;
pub(crate) use problems::{problem_document, problem_response};
pub use problems::{HttpProblem, Problem, ProblemDocument};
pub use profile_authoring::profile_authoring_router;
pub use profiles::{profiles_router, ProfileRoutePolicy, ProfilesState};
pub use triggers::{trigger_router, TriggerRoutesState};

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
    pub(super) standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
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
            standing: Arc::new(layer2_core::account_deletion::AvailableAccounts),
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

async fn openapi_document(headers: HeaderMap) -> Response {
    openapi_cache::document(&headers)
}

async fn agent_manager_openapi_document(headers: HeaderMap) -> Response {
    openapi_cache::agent_manager_document(&headers)
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
mod tests;

pub mod event_workers;
pub use agent_models::{
    agent_models_router, AgentModelCatalog, AgentModelCatalogError, AgentModelRuntimeSupport,
    AgentModelView, AgentModelsView,
};
