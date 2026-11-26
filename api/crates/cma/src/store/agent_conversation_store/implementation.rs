use layer2_core::sandbox::store::AgentMessageCheckpoint;
use layer2_core::sandbox::AgentUpdateOutcome;

use super::*;

#[async_trait::async_trait]
impl AgentConversationStore for MongoSandboxStore {
    async fn commit_turn_timeout(
        &self,
        settlement: &layer2_core::sandbox::turn_timeout::TurnTimeoutSettlement,
    ) -> Result<(), SandboxStoreError> {
        super::super::turn_timeouts::settle(self, settlement).await
    }
    async fn commit_agent_capacity_update(
        &self,
        settlement: layer2_core::sandbox::store::AgentConversationCapacitySettlement,
    ) -> Result<AgentUpdateOutcome, SandboxStoreError> {
        let mut proposal = settlement;
        loop {
            let result = checkpoint::commit(
                self,
                proposal.current().clone(),
                proposal.previous().cloned(),
                proposal.checkpoint().clone(),
                Some(proposal.mutation().clone()),
                checkpoint::Authority::Capacity(Box::new(proposal.clone())),
            )
            .await;
            match result {
                Err(SandboxStoreError::AgentLogCapacityExceeded) => {
                    let Some(compact) = proposal.compact_failure()? else {
                        return result;
                    };
                    proposal = compact;
                }
                _ => return result,
            }
        }
    }

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

    async fn agent_message_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<Option<layer2_core::sandbox::store::AgentMessageSnapshot>, SandboxStoreError> {
        checkpoint::load_snapshot(self, id).await
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
        checkpoint::commit(
            self,
            current,
            previous,
            next,
            mutation,
            checkpoint::Authority::Reopen(Box::new(guard)),
        )
        .await
    }

    async fn commit_agent_message_update(
        &self,
        current: Agent,
        previous: Option<AgentMessageCheckpoint>,
        next: AgentMessageCheckpoint,
        mutation: Option<AgentConversationMutation>,
    ) -> Result<AgentUpdateOutcome, SandboxStoreError> {
        checkpoint::commit(
            self,
            current,
            previous,
            next,
            mutation,
            checkpoint::Authority::Live,
        )
        .await
    }
}
