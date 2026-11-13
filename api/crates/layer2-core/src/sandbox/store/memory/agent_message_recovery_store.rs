use super::*;

#[async_trait::async_trait]
impl AgentConversationStore for LostCommitReply {
    async fn agent_message_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<Option<crate::sandbox::store::AgentMessageSnapshot>, SandboxStoreError> {
        let snapshot = self.store.agent_message_snapshot(id).await?;
        if self.advance_on_read.swap(false, Ordering::SeqCst) {
            let current = self.store.get_agent(id).await?.unwrap();
            let mut next = current.clone();
            next.revision += 1;
            next.conversation.last_seq += 1;
            self.store
                .commit_agent_conversation(AgentConversationMutation {
                    settings_fence: None,
                    command_claim: None,
                    execution_fence: None,
                    trigger_execution_fence: None,
                    admission_guard: None,
                    expected_agent_revision: current.revision,
                    expected_last_seq: current.conversation.last_seq,
                    runtime_generation: 1,
                    agent: next,
                    appended_at: NOW.into(),
                    agent_logs: vec![AgentLogKind::ConversationGap { frames: 1 }],
                })
                .await?;
        }
        Ok(snapshot)
    }
    async fn agent_message_checkpoint(
        &self,
        id: &AgentId,
    ) -> Result<Option<AgentMessageCheckpoint>, SandboxStoreError> {
        self.store.agent_message_checkpoint(id).await
    }
    async fn commit_agent_message_update(
        &self,
        current: Agent,
        previous: Option<AgentMessageCheckpoint>,
        next: AgentMessageCheckpoint,
        mutation: Option<AgentConversationMutation>,
    ) -> Result<AgentUpdateOutcome, SandboxStoreError> {
        if self.publish_on_commit.swap(false, Ordering::SeqCst) {
            anchor_tests::publish_connection(&self.store, &current.id).await;
        }
        let outcome = self
            .store
            .commit_agent_message_update(current, previous, next, mutation)
            .await?;
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(SandboxStoreError::Unavailable(
                "simulated lost commit reply".into(),
            ));
        }
        Ok(outcome)
    }
    async fn commit_agent_conversation(
        &self,
        mutation: AgentConversationMutation,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.store.commit_agent_conversation(mutation).await
    }
    async fn pending_agent_approvals(
        &self,
        id: &AgentId,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.store.pending_agent_approvals(id).await
    }
}
