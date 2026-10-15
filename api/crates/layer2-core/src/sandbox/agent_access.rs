//! Owner-approved access requests from a running Agent.
use crate::{resource_access::CallerContext, AgentId, Caller};
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
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentAccessError {
    #[error("this access request is not available to the caller")]
    NotFound,
    #[error("sign in as the Agent owner to approve service access")]
    Forbidden,
    #[error("the access request is invalid")]
    Invalid,
    #[error("this access request has already changed")]
    Conflict,
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
