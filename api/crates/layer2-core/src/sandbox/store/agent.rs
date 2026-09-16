use async_trait::async_trait;

use crate::ids::{AgentId, SubjectId};
use crate::sandbox::Agent;

use super::{Page, PageRequest, SandboxStoreError};

#[async_trait]
pub trait AgentStore: Send + Sync {
    /// Readiness may allow replacement of an expired undispatched claim, but
    /// never grants execution authority. Unsupported stores retain queued work.
    async fn trigger_ready_for_claim(
        &self,
        _command: &crate::sandbox::agent_io::AdmittedAgentCommand,
    ) -> Result<(), SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "Trigger command readiness is unavailable".into(),
        ))
    }

    /// Unsupported store compositions cannot authorize Trigger command execution.
    async fn trigger_execution_proof(
        &self,
        _command: &crate::sandbox::agent_io::AdmittedAgentCommand,
    ) -> Result<crate::sandbox::agent_io::TriggerExecutionProof, SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "Trigger command execution is unavailable".into(),
        ))
    }
    /// Must verify both the Agent sidecar and permanent binding before a legacy write.
    async fn require_legacy_first_message(&self, _agent: &Agent) -> Result<(), SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "first-message protocol guard is unavailable".into(),
        ))
    }

    async fn create_agent(&self, agent: Agent) -> Result<(), SandboxStoreError>;
    async fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>, SandboxStoreError>;
    async fn save_agent(
        &self,
        expected_revision: u64,
        agent: Agent,
    ) -> Result<(), SandboxStoreError>;
    async fn list_agents(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError>;
}
