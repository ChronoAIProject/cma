//! Narrow runtime tool ports: requesting owner consent grants no service scope.
use super::agent_access::{validate_access_services, AgentAccessError, AgentAccessRequest};
use super::{AgentCreationProvenance, RuntimeBinding};
use crate::{AgentId, SandboxId, SubjectId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAccessRequester {
    pub agent_id: AgentId,
    pub owner: SubjectId,
    pub sandbox_id: SandboxId,
    pub conversation: String,
    pub runtime: RuntimeBinding,
    pub creation: AgentCreationProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAccessProposal {
    pub request_key: String,
    pub services: Vec<String>,
    pub reason: String,
}

impl AgentAccessProposal {
    pub fn validate(&self) -> Result<(), AgentAccessError> {
        validate_access_services(&self.services)?;
        if self.request_key.is_empty()
            || self.request_key.len() > 128
            || !self.request_key.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
            || self.reason.trim().is_empty()
            || self.reason.len() > 2048
            || self
                .reason
                .chars()
                .any(|value| value.is_control() && value != '\n' && value != '\t')
            || self
                .services
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.services.len()
        {
            return Err(AgentAccessError::Invalid);
        }
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait AgentAccessToolAuthenticator: Send + Sync {
    /// Pure protocol discovery before the native conversation is confirmed.
    async fn authenticate_setup(&self, capability: &str) -> Result<(), AgentAccessError>;
    /// A confirmed, currently owned Agent and exact original native conversation.
    async fn authenticate(
        &self,
        capability: &str,
    ) -> Result<AgentAccessRequester, AgentAccessError>;
}

#[async_trait::async_trait]
pub trait AgentAccessToolAdmission: Send + Sync {
    /// Revalidate current requester authority when recording the request.
    /// Retrying the same Agent/request_key must recover the same proposal.
    async fn request(
        &self,
        requester: AgentAccessRequester,
        proposal: AgentAccessProposal,
    ) -> Result<AgentAccessRequest, AgentAccessError>;
}
