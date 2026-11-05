use layer2_core::sandbox::store::{AgentMessageCheckpoint, PendingAgentApprovalPage};
use layer2_core::sandbox::AgentUpdateOutcome;

use super::*;

#[async_trait::async_trait]
impl AgentConversationStore for MongoSandboxStore {
    async fn pending_agent_approvals_page(
        &self,
        id: &AgentId,
        through_seq: u64,
        after: u64,
        limit: u16,
    ) -> Result<PendingAgentApprovalPage, SandboxStoreError> {
        let mut session = snapshot_session(self, None).await?;
        let agent = self
            .agents
            .find_one(doc! {"_id":id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?
            .as_ref()
            .map(read_agent)
            .transpose()?
            .ok_or(SandboxStoreError::NotFound)?;
        if agent.conversation.last_seq != through_seq {
            return Err(SandboxStoreError::Conflict);
        }
        if after > through_seq {
            return Err(SandboxStoreError::InvalidPage);
        }
        if let Some(page) = crate::store::agent_log_segments::approval_pages::page(
            self,
            &agent,
            after,
            limit,
            &mut session,
        )
        .await
        .map_err(crate::store::agent_log_segments::approvals::store_error)?
        {
            return Ok(page);
        }
        let entries = pending_approvals_snapshot(self, id, &mut session).await?;
        PendingAgentApprovalPage::from_entries(entries, after, limit)
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

    async fn pending_agent_approval(
        &self,
        id: &AgentId,
        approval_id: &str,
    ) -> Result<(Option<AgentLogEntry>, bool), SandboxStoreError> {
        let mut session = snapshot_session(self, None).await?;
        let agent = self
            .agents
            .find_one(doc! {"_id":id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?
            .as_ref()
            .map(read_agent)
            .transpose()?
            .ok_or(SandboxStoreError::NotFound)?;
        if let Some(result) = crate::store::agent_log_segments::approvals::exact(
            self,
            &agent,
            approval_id,
            &mut session,
            true,
        )
        .await
        .map_err(crate::store::agent_log_segments::approvals::store_error)?
        {
            return Ok(result);
        }
        let mut found = None;
        let mut other = false;
        for entry in pending_approvals_snapshot(self, id, &mut session).await? {
            if matches!(&entry.kind, layer2_core::sandbox::AgentLogKind::ApprovalRequested { approval_id: key, .. } if key == approval_id)
            {
                found = Some(entry);
            } else {
                other = true;
            }
        }
        Ok((found, other))
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
