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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<AgentLifecycleImageReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<AgentLifecycleRepository>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<AgentLifecycleProfile>,
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
/// detailed sandbox events, while CMA exposes the three user-facing gates.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AgentLifecycleEventView {
    pub sequence: u64,
    pub phase: String,
    pub status: String,
    pub message: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AgentLifecycleOperationView {
    pub id: String,
    pub status: String,
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub events: Vec<AgentLifecycleEventView>,
}

#[derive(Debug, Error)]
pub enum AgentLifecycleOperationReadError {
    #[error("the Agent lifecycle operation does not exist")]
    NotFound,
    #[error("the authenticated caller cannot read this Agent lifecycle operation")]
    Forbidden,
    #[error("the Agent lifecycle operation store is unavailable")]
    Unavailable,
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
        };
        let body = super::problem_document(status, slug, &self.to_string(), code);
        super::problem_response(status, body)
    }
}

/// Read port for durable lifecycle status and phase events.
#[async_trait]
pub trait AgentLifecycleOperationReader: Send + Sync {
    async fn read(
        &self,
        caller: &Caller,
        operation_id: &str,
    ) -> Result<AgentLifecycleOperationView, AgentLifecycleOperationReadError>;
}

#[derive(Debug, Error)]
pub enum AgentLifecycleProvisioningError {
    #[error("the Agent Manager lifecycle implementation is unavailable")]
    Unavailable,
    #[error("the authenticated caller cannot create an Agent")]
    Forbidden,
    #[error("invalid Agent lifecycle request: {0}")]
    Invalid(String),
    #[error("the selected services need to be connected before provisioning")]
    ConnectionRequired {
        services: Vec<String>,
        connection_target: Value,
    },
    #[error("Agent lifecycle request conflicts with existing state: {0}")]
    Conflict(String),
}

impl IntoResponse for AgentLifecycleProvisioningError {
    fn into_response(self) -> Response {
        let (status, slug, code, detail) = match &self {
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
            Self::Conflict(_) => (
                StatusCode::CONFLICT,
                "agent-manager-conflict",
                "agent_busy",
                self.to_string(),
            ),
        };
        let mut body = super::problem_document(status, slug, &detail, code);
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
    async fn create(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        delegated_credential: Option<&str>,
        request: AgentLifecycleCreateRequest,
        idempotency_key: &str,
    ) -> Result<AgentLifecycleCreateReceipt, AgentLifecycleProvisioningError>;
}
