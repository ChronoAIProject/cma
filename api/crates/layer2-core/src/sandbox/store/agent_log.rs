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

    /// Prove the latest started turn through the captured Agent head. Stores
    /// with a trusted checkpoint may supply the same fact in bounded work.
    /// Dispatch must still recheck the predecessor in its append transaction.
    async fn agent_latest_started_turn(
        &self,
        agent: &Agent,
    ) -> Result<Option<u64>, SandboxStoreError> {
        let mut after = 0;
        let mut latest = None;
        while after < agent.conversation.last_seq {
            let limit = (agent.conversation.last_seq - after).min(u64::from(MAX_PAGE_SIZE)) as u16;
            let entries = self.agent_log(&agent.id, after, limit).await?;
            if entries.len() != usize::from(limit) {
                return Err(SandboxStoreError::SequenceConflict);
            }
            for entry in entries {
                after += 1;
                if entry.agent_id != agent.id || entry.seq != after {
                    return Err(SandboxStoreError::SequenceConflict);
                }
                if let AgentLogKind::TurnStarted { turn_sequence, .. } = entry.kind {
                    if turn_sequence != entry.seq {
                        return Err(SandboxStoreError::SequenceConflict);
                    }
                    latest = Some(turn_sequence);
                }
            }
        }
        Ok(latest)
    }

    /// Read one bounded page from a frozen high-water range while validating
    /// the backing log in one store operation. Implementations must return
    /// exactly the available entries in `(after, through]`, up to `limit`, or
    /// a sequence conflict when that range is not complete.
    async fn agent_log_range(
        &self,
        agent_id: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE || after > through {
            return Err(SandboxStoreError::InvalidPage);
        }
        let expected = (through - after).min(u64::from(limit)) as usize;
        if expected == 0 {
            return Ok(Vec::new());
        }
        let entries = self.agent_log(agent_id, after, expected as u16).await?;
        if entries.len() != expected
            || entries.iter().enumerate().any(|(offset, entry)| {
                entry.agent_id != *agent_id || entry.seq != after + offset as u64 + 1
            })
        {
            return Err(SandboxStoreError::SequenceConflict);
        }
        Ok(entries)
    }

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
