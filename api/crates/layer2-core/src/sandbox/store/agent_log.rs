//! Durable Agent history reads and append operations.

use super::*;

#[async_trait]
pub trait AgentLogStore: Send + Sync {
    /// Current controls and one pending-approval page under the exact captured
    /// aggregate. None selects only the bounded legacy fold.
    async fn agent_log_current_state(
        &self,
        _agent: &Agent,
        _after_approval: u64,
        _limit: u16,
    ) -> Result<Option<AgentLogCurrentState>, SandboxStoreError> {
        Ok(None)
    }
    /// Active turn at an arbitrary retained global sequence. Segmented stores
    /// prove both indexed neighbors so a missing turn cannot look like a gap.
    async fn agent_log_turn_at(
        &self,
        _agent: &Agent,
        _through: u64,
    ) -> Result<super::super::AgentLogTurnLookup, SandboxStoreError> {
        Ok(super::super::AgentLogTurnLookup::Legacy)
    }

    /// None denotes a legacy Agent only. An activated Agent with missing,
    /// unsupported or stale evidence must return an error, never fall back.
    async fn agent_log_projection(
        &self,
        _agent: &Agent,
    ) -> Result<Option<super::super::AgentLogProjectionState>, SandboxStoreError> {
        Ok(None)
    }

    /// Exact message, turn and terminal evidence for final-answer and feedback
    /// authority, sharing one captured snapshot and one active-segment scan.
    async fn agent_log_final_answer(
        &self,
        _agent: &Agent,
        _sequence: u64,
    ) -> Result<super::super::AgentLogAnswerLookup, SandboxStoreError> {
        Ok(super::super::AgentLogAnswerLookup::Legacy)
    }

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

    /// Stream one validated source interval into a bounded projection. Mongo
    /// overrides this to retain only one raw BSON row at a time. The visitor is
    /// called for every row, even after its output byte budget has been filled.
    async fn visit_agent_log_range(
        &self,
        agent_id: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
        visit: &mut (dyn FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send),
    ) -> Result<(), SandboxStoreError> {
        for entry in self
            .agent_log_range(agent_id, after, through, limit)
            .await?
        {
            visit(entry)?;
        }
        Ok(())
    }

    /// Stream a page and, when present, its one immediate predecessor. This
    /// lets replay bind steering input without a full active scan per message.
    /// The predecessor is evidence only and is never a delivered page entry.
    async fn visit_agent_log_page_with_predecessor(
        &self,
        agent_id: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
        visit: &mut (dyn FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send),
    ) -> Result<(), SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE || after > through {
            return Err(SandboxStoreError::InvalidPage);
        }
        if after > 0 {
            let previous = self
                .agent_log_entry(agent_id, after)
                .await?
                .ok_or(SandboxStoreError::SequenceConflict)?;
            visit(previous)?;
        }
        self.visit_agent_log_range(agent_id, after, through, limit, visit)
            .await
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
    async fn visit_agent_log_prefix(
        &self,
        agent_id: &AgentId,
        through_seq: u64,
        visit: &mut (dyn FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send),
    ) -> Result<(), SandboxStoreError> {
        if through_seq > 10_128 {
            return Err(SandboxStoreError::InvalidPage);
        }
        for entry in self.agent_log_prefix(agent_id, through_seq).await? {
            visit(entry)?;
        }
        Ok(())
    }

    /// Legacy bounded prefix compatibility. Unlimited consumers use pages or
    /// indexed semantic evidence, never this materializing interface.
    async fn agent_log_prefix(
        &self,
        agent_id: &AgentId,
        through_seq: u64,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        if through_seq > 10_128 {
            return Err(SandboxStoreError::InvalidPage);
        }
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

pub struct AgentLogCurrentState {
    pub context: Vec<AgentLogEntry>,
    pub approvals: super::PendingAgentApprovalPage,
}
