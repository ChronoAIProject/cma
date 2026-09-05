use async_trait::async_trait;

use crate::ids::AgentId;
use crate::sandbox::{Agent, AgentLogEntry, AgentLogKind};

use super::SandboxStoreError;

/// One generation-fenced, all-or-nothing conversation mutation.
#[derive(Debug, Clone)]
pub struct AgentConversationMutation {
    pub expected_agent_revision: u64,
    pub expected_last_seq: u64,
    pub runtime_generation: u64,
    pub agent: Agent,
    pub appended_at: String,
    pub agent_logs: Vec<AgentLogKind>,
}

/// Durable agent conversation state and its gap-free semantic log.
#[async_trait]
pub trait AgentConversationStore: Send + Sync {
    async fn commit_agent_conversation(
        &self,
        mutation: AgentConversationMutation,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;

    async fn pending_agent_approvals(
        &self,
        id: &AgentId,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;
}
