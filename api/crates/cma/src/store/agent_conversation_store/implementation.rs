use layer2_core::sandbox::store::AgentMessageCheckpoint;
use layer2_core::sandbox::AgentUpdateOutcome;

use super::*;

#[async_trait::async_trait]
impl AgentConversationStore for MongoSandboxStore {
    async fn commit_agent_conversation(
        &self,
        mutation: AgentConversationMutation,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        commit_conversation(self, mutation).await
    }

    async fn pending_agent_approvals(
        &self,
        id: &AgentId,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        pending_approvals(self, id).await
    }

    async fn agent_message_checkpoint(
        &self,
        id: &AgentId,
    ) -> Result<Option<AgentMessageCheckpoint>, SandboxStoreError> {
        checkpoint::load(self, id).await
    }

    async fn commit_agent_reopen_update(
        &self,
        current: Agent,
        previous: Option<AgentMessageCheckpoint>,
        next: AgentMessageCheckpoint,
        mutation: Option<AgentConversationMutation>,
        guard: layer2_core::sandbox::store::AgentReopenGuard,
    ) -> Result<AgentUpdateOutcome, SandboxStoreError> {
        checkpoint::commit(self, current, previous, next, mutation, Some(guard)).await
    }

    async fn commit_agent_message_update(
        &self,
        current: Agent,
        previous: Option<AgentMessageCheckpoint>,
        next: AgentMessageCheckpoint,
        mutation: Option<AgentConversationMutation>,
    ) -> Result<AgentUpdateOutcome, SandboxStoreError> {
        checkpoint::commit(self, current, previous, next, mutation, None).await
    }
}
