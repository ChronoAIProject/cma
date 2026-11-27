//! The user-facing Agent Manager lifecycle request.
//!
//! The compatibility Agent routes still expose the retained hierarchy, but the
//! lifecycle entry point deliberately accepts only an inline request.  A
//! trusted composition supplies the provisioning implementation through the
//! `AgentLifecycleProvisioner` port below.

use async_trait::async_trait;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use layer2_core::{resource_access::CallerContext, Caller};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use utoipa::ToSchema;

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecycleCreateRequest {
    pub name: String,
    /// Omission retains the named-title behavior of existing clients and requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_mode: Option<AgentLifecycleTitleMode>,
    /// Exact initial task, at most 262144 UTF-8 bytes and not whitespace-only.
    /// Replaces the readiness prompt after preparation; omission retains setup.
    /// Repository tasks prepare an immutable source checkout in a new Sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_message: Option<String>,
    /// Versioned recommended task inputs, verified before lifecycle admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<AgentLifecycleTaskTemplate>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub desktop: bool,
    /// Provision a desktop-capable Linux runtime for this Agent.
    ///
    /// Desktop runtimes are immutable Sandbox configuration.  The option is
    /// therefore supported only when this request creates a new Sandbox.
    /// Omit the option when adding an Agent to an existing Sandbox or Workspace;
    /// those requests retain the parent's immutable desktop configuration.
    #[serde(default)]
    pub machine_included: bool,
    /// Add an Agent to this owned, ready Workspace using its existing Sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Create an empty Workspace and Agent inside this owned Sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<AgentLifecycleImageReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<AgentLifecycleRepository>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<AgentLifecycleProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleTitleMode {
    Automatic,
    Named,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecycleTaskTemplate {
    pub id: String,
    pub version: u32,
    /// Exact required field set for the selected template; unknown fields are refused.
    pub fields: std::collections::BTreeMap<String, String>,
}

impl AgentLifecycleCreateRequest {
    pub fn validate_template(&self) -> Result<(), AgentLifecycleProvisioningError> {
        use layer2_core::sandbox::task_template::{compile_task_template, TaskTemplateError};
        let Some(template) = &self.template else {
            return Ok(());
        };
        let compiled = compile_task_template(&template.id, template.version, &template.fields)
            .map_err(AgentLifecycleProvisioningError::Template)?;
        if self.initial_message.as_deref() != Some(compiled.as_str()) {
            return Err(AgentLifecycleProvisioningError::Template(
                TaskTemplateError::new("task_template_message_mismatch", Some("initial_message")),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecycleImageReference {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecycleRepository {
    pub url: String,
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Credential scope required by the repository work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<GitHubRepositoryNeed>,
    /// Select one connection from the owner's GitHub inventory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitHubRepositoryNeed {
    PublicRepositoryWrite,
    PrivateRepositoryRead,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecycleProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub skills: Vec<AgentLifecycleSkill>,
    #[serde(default)]
    pub permissions: AgentLifecyclePermissions,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecycleSkill {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecyclePermissions {
    #[serde(default)]
    pub services: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AgentLifecycleCreateReceipt {
    pub id: String,
    pub status: String,
}

/// A durable lifecycle event exposed to Agent Manager clients.  The event
/// stream is intentionally small and phase-oriented: the runtime owns the
/// detailed sandbox events, while CMA exposes observed provisioning stages.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AgentLifecycleEventView {
    pub sequence: u64,
    pub phase: String,
    pub status: String,
    pub message: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AgentLifecycleConnectionRequired {
    pub services: Vec<String>,
    pub target: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_access: Option<RepositoryAccessRefusal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_scope_repair: Option<RepositoryScopeRepair>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RepositoryScopeRepair {
    pub connection_id: String,
    pub operation: GitHubRepositoryNeed,
    pub required_scope: String,
    pub manage_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryAccessRefusal {
    VerificationUnavailable,
    InsufficientScope,
    MissingConnection,
    RepositoryUnavailable,
    ReferenceUnavailable,
    RuntimeUnsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum AgentLifecycleFailure {
    SandboxLimitReached {
        limit: u16,
        live: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schema(value_type = Option<Object>)]
        quota_usage: Option<layer2_core::sandbox::SandboxQuotaUsage>,
    },
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AgentLifecycleOperationView {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<String>,
    pub status: String,
    pub phase: String,
    /// The owner can request cancellation before allocation. The write checks
    /// the operation and its creation proof again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_cancel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Omitted without an Agent ID or when observation fails; null for a missing or Deleted Agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, nullable = true)]
    pub callback_url: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<AgentLifecycleFailure>,
    pub events: Vec<AgentLifecycleEventView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<AgentLifecycleConnectionRequired>,
}

#[derive(Debug, Error)]
pub enum AgentLifecycleOperationReadError {
    #[error("the Agent lifecycle operation does not exist")]
    NotFound,
    #[error("the authenticated caller cannot read this Agent lifecycle operation")]
    Forbidden,
    #[error("the Agent lifecycle operation store is unavailable")]
    Unavailable,
    #[error("{0}")]
    Conflict(String),
}

impl IntoResponse for AgentLifecycleOperationReadError {
    fn into_response(self) -> Response {
        let (status, slug, code) = match self {
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "agent-lifecycle-operation-not-found",
                "not_found",
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "agent-lifecycle-operation-forbidden",
                "permission_denied",
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "agent-lifecycle-operation-unavailable",
                "temporarily_unavailable",
            ),
            Self::Conflict(_) => (
                StatusCode::CONFLICT,
                "agent-lifecycle-operation-conflict",
                "creation_cancellation_unavailable",
            ),
        };
        let body = super::problem_document(status, slug, &self.to_string(), code);
        super::problem_response(status, body)
    }
}

/// Owner-scoped operation status, phase events and cancellation before allocation.
#[async_trait]
pub trait AgentLifecycleOperationReader: Send + Sync {
    async fn cancel_unallocated(
        &self,
        _caller: &Caller,
        _operation_id: &str,
    ) -> Result<AgentLifecycleOperationView, AgentLifecycleOperationReadError> {
        Err(AgentLifecycleOperationReadError::Unavailable)
    }
    async fn read(
        &self,
        caller: &Caller,
        operation_id: &str,
    ) -> Result<AgentLifecycleOperationView, AgentLifecycleOperationReadError>;
}

#[derive(Debug, Error)]
pub enum AgentLifecycleProvisioningError {
    #[error("The selected task template has invalid inputs.")]
    Template(layer2_core::sandbox::task_template::TaskTemplateError),
    #[error("sandbox limit reached: {live} of {limit}")]
    SandboxLimitReached {
        limit: u16,
        live: u64,
        quota_usage: Option<layer2_core::sandbox::SandboxQuotaUsage>,
    },
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
    #[error("desktop is unavailable: {0}")]
    DesktopUnsupported(String),
    #[error("the Agent Manager lifecycle implementation is unavailable")]
    Unavailable,
    #[error("the authenticated caller cannot create an Agent")]
    Forbidden,
    #[error("sign in again to authorize the selected Image Manager image")]
    SignInRequired,
    #[error("the selected Image Manager image is not accessible to this account")]
    ImageAccessDenied,
    #[error("new custom image selections are unavailable")]
    ImageSelectionUnsupported,
    #[error("invalid Agent lifecycle request: {0}")]
    Invalid(String),
    #[error("the selected services need to be connected before provisioning")]
    ConnectionRequired {
        services: Vec<String>,
        connection_target: Value,
    },
    #[error("GitHub repository access could not be verified")]
    RepositoryAccessRequired {
        connection_target: Value,
        reason: RepositoryAccessRefusal,
        repair: Option<Box<RepositoryScopeRepair>>,
    },
    #[error("Agent lifecycle request conflicts with existing state: {0}")]
    Conflict(String),
}

impl IntoResponse for AgentLifecycleProvisioningError {
    fn into_response(self) -> Response {
        let (status, slug, code, detail) = match &self {
            Self::AdmissionPaused => return super::admission_problem::paused(),
            Self::Template(error) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "task-template-invalid",
                error.code,
                self.to_string(),
            ),
            Self::SandboxLimitReached {
                limit,
                live,
                quota_usage,
            } => {
                let status = StatusCode::TOO_MANY_REQUESTS;
                let mut body = super::problem_document(
                    status,
                    "resource-limit-reached",
                    "The owner has reached the live resource limit.",
                    "sandbox_limit_reached",
                );
                body["limit"] = (*limit).into();
                body["live"] = (*live).into();
                if let Some(usage) = quota_usage {
                    body["quota_usage"] = super::sandbox_capacity::quota_usage_json(*usage);
                }
                return super::problem_response(status, body);
            }
            Self::DesktopUnsupported(_) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "agent-manager-desktop-unsupported",
                "desktop_unsupported",
                self.to_string(),
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "agent-manager-unavailable",
                "temporarily_unavailable",
                self.to_string(),
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "agent-manager-forbidden",
                "permission_denied",
                self.to_string(),
            ),
            Self::SignInRequired => (
                StatusCode::UNAUTHORIZED,
                "agent-manager-sign-in-required",
                "not_authenticated",
                self.to_string(),
            ),
            Self::ImageAccessDenied => (
                StatusCode::FORBIDDEN,
                "agent-manager-image-access-denied",
                "permission_denied",
                self.to_string(),
            ),
            Self::ImageSelectionUnsupported => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "agent-manager-image-selection-unsupported",
                "image_selection_unsupported",
                self.to_string(),
            ),
            Self::Invalid(_) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "agent-manager-invalid-request",
                "invalid_request",
                self.to_string(),
            ),
            Self::ConnectionRequired { .. } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "agent-manager-connection-required",
                "connection_required",
                self.to_string(),
            ),
            Self::RepositoryAccessRequired { .. } => (
                StatusCode::SERVICE_UNAVAILABLE,
                "agent-manager-repository-verification-unavailable",
                "repository_verification_unavailable",
                self.to_string(),
            ),
            Self::Conflict(_) => (
                StatusCode::CONFLICT,
                "agent-manager-conflict",
                "agent_busy",
                self.to_string(),
            ),
        };
        let mut body = super::problem_document(status, slug, &detail, code);
        if let Self::Template(error) = &self {
            if let Some(field) = error.field {
                body["field"] = serde_json::json!(field);
            }
        }
        if let Self::ConnectionRequired {
            services,
            connection_target,
        } = self
        {
            if let Some(object) = body.as_object_mut() {
                object.insert(
                    "services".into(),
                    serde_json::to_value(services).expect("service identifiers are JSON values"),
                );
                object.insert("connection_target".into(), connection_target);
            }
        }
        super::problem_response(status, body)
    }
}

/// Provisioning port implemented by the composed Agent Manager.
#[async_trait]
pub trait AgentLifecycleProvisioner: Send + Sync {
    async fn create_with_submission(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        delegated_credential: Option<&str>,
        request: AgentLifecycleCreateRequest,
        idempotency_key: &str,
        submission: super::creation_outcomes::Submission,
    ) -> Result<AgentLifecycleCreateReceipt, AgentLifecycleProvisioningError> {
        if submission.fence.is_some() || submission.replay_operation_id.is_some() {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        self.create(
            caller,
            context,
            delegated_credential,
            request,
            idempotency_key,
        )
        .await
    }
    async fn desktop_capability(
        &self,
        _caller: &Caller,
        _context: Option<&CallerContext>,
        _scope: super::DesktopCreationScope,
    ) -> Result<
        layer2_core::sandbox::desktop_creation::DesktopCapability,
        AgentLifecycleProvisioningError,
    > {
        Ok(
            layer2_core::sandbox::desktop_creation::DesktopCapability::unavailable(
                "This service does not support desktop creation.",
            ),
        )
    }

    async fn create(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        delegated_credential: Option<&str>,
        request: AgentLifecycleCreateRequest,
        idempotency_key: &str,
    ) -> Result<AgentLifecycleCreateReceipt, AgentLifecycleProvisioningError>;
}
