use super::*;

#[async_trait]
impl AgentConversationStore for RejectingConversationStore {
    async fn agent_message_checkpoint(
        &self,
        id: &AgentId,
    ) -> Result<Option<crate::sandbox::store::AgentMessageCheckpoint>, SandboxStoreError> {
        self.inner.agent_message_checkpoint(id).await
    }

    async fn commit_agent_message_update(
        &self,
        current: Agent,
        previous: Option<crate::sandbox::store::AgentMessageCheckpoint>,
        next: crate::sandbox::store::AgentMessageCheckpoint,
        mutation: Option<AgentConversationMutation>,
    ) -> Result<crate::sandbox::AgentUpdateOutcome, SandboxStoreError> {
        self.commit_attempts.fetch_add(1, Ordering::SeqCst);
        if self.reject_commits.load(Ordering::SeqCst) {
            return Err(SandboxStoreError::Unavailable("write unavailable".into()));
        }
        self.inner
            .commit_agent_message_update(current, previous, next, mutation)
            .await
    }

    async fn commit_agent_conversation(
        &self,
        mutation: AgentConversationMutation,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.commit_attempts.fetch_add(1, Ordering::SeqCst);
        if self.conflict_commits.load(Ordering::SeqCst) {
            return Err(SandboxStoreError::Conflict);
        }
        if self.reject_commits.load(Ordering::SeqCst) {
            return Err(SandboxStoreError::Unavailable("write unavailable".into()));
        }
        self.inner.commit_agent_conversation(mutation).await
    }

    async fn pending_agent_approvals(
        &self,
        id: &AgentId,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.inner.pending_agent_approvals(id).await
    }
}
