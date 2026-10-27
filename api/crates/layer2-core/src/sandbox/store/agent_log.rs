//! Durable Agent history reads and append operations.

use super::*;

#[async_trait]
pub trait AgentLogStore: Send + Sync {
    async fn append_agent_log(
        &self,
        agent_id: &AgentId,
        appended_at: String,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError>;
    async fn agent_log(
        &self,
        agent_id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;

    /// Read one immutable entry for message metadata. The caller must first
    /// authorize the owning lineage and bound sequence by the captured Agent's
    /// committed high-water mark. Conversation replay still uses validated scans.
    async fn agent_log_entry(
        &self,
        agent_id: &AgentId,
        sequence: u64,
    ) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
        if sequence == 0 {
            return Ok(None);
        }
        Ok(self
            .agent_log(agent_id, sequence - 1, 1)
            .await?
            .into_iter()
            .next())
    }

    /// Read the exact prefix needed for a captured Agent projection.
    async fn agent_log_prefix(
        &self,
        agent_id: &AgentId,
        through_seq: u64,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        let mut entries = Vec::new();
        let mut after = 0;
        while after < through_seq {
            let limit = (through_seq - after).min(u64::from(MAX_PAGE_SIZE)) as u16;
            let page = self.agent_log(agent_id, after, limit).await?;
            if page.len() != usize::from(limit) {
                return Err(SandboxStoreError::SequenceConflict);
            }
            after += u64::from(limit);
            entries.extend(page);
        }
        Ok(entries)
    }
}
