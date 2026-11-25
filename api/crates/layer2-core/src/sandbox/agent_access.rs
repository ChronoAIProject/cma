//! Owner-approved access requests from a running Agent.
use crate::{resource_access::CallerContext, AgentId, Caller, SandboxId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAccessStatus {
    Pending,
    Connecting,
    Applying,
    Succeeded,
    Denied,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAccessRequest {
    pub id: String,
    pub agent_id: AgentId,
    pub status: AgentAccessStatus,
    pub services: Vec<String>,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_approval: Option<GithubApprovalContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubApprovalContext {
    pub sandbox_id: SandboxId,
    pub sandbox_name: String,
    pub other_agents: Vec<AgentId>,
    pub current_user_service_id: Option<String>,
    pub scope_revision: u64,
    pub credential_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubConnectionApproval {
    pub sandbox_id: SandboxId,
    pub user_service_id: String,
    pub scope_revision: u64,
    pub credential_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentAccessError {
    #[error("account deletion is in progress")]
    AccountDeletionInProgress,
    #[error("this access request is not available to the caller")]
    NotFound,
    #[error("the caller cannot manage this Agent's service access")]
    Forbidden,
    #[error("use a direct owner access token or owner sign-in; runtime and delegated credentials cannot manage service access")]
    OwnerAuthorityRequired,
    #[error("connect the requested services and authorize their backing resources in the identity provider, then retry with an owner access token")]
    ServiceConnectionRequired,
    #[error("the access request is invalid")]
    Invalid,
    #[error("this access request has already changed")]
    Conflict,
    #[error("GitHub access changed before approval. Review the current Sandbox and choose a connection again.")]
    StaleGithubApproval,
    #[error("service access is temporarily unavailable")]
    Unavailable,
}

/// Public interfaces project this durable service; a tool can only request
/// access. Only an authenticated owner command may authorize a scope change.
#[async_trait::async_trait]
pub trait AgentAccessRequests: Send + Sync {
    async fn list(
        &self,
        caller: &Caller,
        agent: &AgentId,
    ) -> Result<Vec<AgentAccessRequest>, AgentAccessError>;
    async fn approve(
        &self,
        context: &CallerContext,
        credential: &str,
        agent: &AgentId,
        request: &str,
        services: Vec<String>,
        key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError>;
    #[allow(clippy::too_many_arguments)]
    async fn approve_github_connection(
        &self,
        _context: &CallerContext,
        _credential: &str,
        _agent: &AgentId,
        _request: &str,
        _services: Vec<String>,
        _choice: GithubConnectionApproval,
        _key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        Err(AgentAccessError::Unavailable)
    }
    async fn replace_services(
        &self,
        _context: &CallerContext,
        _credential: &str,
        _agent: &AgentId,
        _services: Vec<String>,
        _expected_services: Vec<String>,
        _key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        Err(AgentAccessError::Unavailable)
    }
    async fn deny(
        &self,
        caller: &Caller,
        agent: &AgentId,
        request: &str,
        key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError>;
}

pub fn validate_access_services(services: &[String]) -> Result<(), AgentAccessError> {
    if services.is_empty()
        || services.len() > 32
        || services.iter().any(|service| {
            service.is_empty()
                || service.len() > 128
                || !service
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
    {
        return Err(AgentAccessError::Invalid);
    }
    Ok(())
}
