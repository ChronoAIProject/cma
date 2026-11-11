//! Durable Agent history reads and append operations.

use super::*;

#[async_trait]
pub trait AgentLogStore: Send + Sync {
    /// Publish one exact durable operation. Implementations without keyed
    /// receipts refuse; silently delegating to unkeyed append would discard
    /// retry identity and could repeat an uncertain publication.
    async fn append_agent_log_keyed(
        &self,
        _agent_id: &AgentId,
        _identity: &AgentLogPublicationIdentity,
        _kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "keyed Agent log publication is not supported by this store".into(),
        ))
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

    /// Read only rows that can change the public transcript for a captured
    /// Agent. `None` means the store cannot prove a bounded projection for the
    /// requested format, so callers must use the complete compatibility read.
    /// The returned rows retain their durable sequence numbers and ordering;
    /// the existing transcript fold remains the semantic authority.
    async fn agent_log_transcript(
        &self,
        agent: &Agent,
    ) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError> {
        // The in-memory compatibility store is used by the unit parity suite.
        // Keep its projection selector honest without making the production
        // default claim a durable proof it cannot provide; Mongo supplies the
        // trusted implementation above this trait boundary.
        #[cfg(test)]
        {
            let entries = self
                .agent_log_prefix(&agent.id, agent.conversation.last_seq)
                .await?;
            return Ok(Some(
                entries
                    .into_iter()
                    .filter(|entry| {
                        matches!(
                            entry.kind,
                            AgentLogKind::Created { .. }
                                | AgentLogKind::ConversationOpened { .. }
                                | AgentLogKind::RuntimeGenerationRetired { .. }
                                | AgentLogKind::TurnStarted { .. }
                                | AgentLogKind::TurnMetadataSet { .. }
                                | AgentLogKind::OwnerMessage { .. }
                                | AgentLogKind::AgentMessage { .. }
                                | AgentLogKind::TurnCompleted { .. }
                                | AgentLogKind::TurnFailed { .. }
                                | AgentLogKind::TurnInterrupted { .. }
                                | AgentLogKind::ConversationGap { .. }
                                | AgentLogKind::OperationDeliveryChanged {
                                    operation: AgentOperationKind::Steer,
                                    state: AgentOperationDelivery::DispatchFenced,
                                    ..
                                }
                        )
                    })
                    .collect(),
            ));
        }
        #[cfg(not(test))]
        let _ = agent;
        Ok(None)
    }

    /// Read only rows that can change usage through a captured Agent head.
    /// `None` means the store cannot prove a bounded usage projection for the
    /// requested format, so callers must use the complete compatibility read.
    async fn agent_log_usage(
        &self,
        agent: &Agent,
    ) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError> {
        #[cfg(test)]
        {
            let entries = self
                .agent_log_prefix(&agent.id, agent.conversation.last_seq)
                .await?;
            return Ok(Some(
                entries
                    .into_iter()
                    .filter(|entry| {
                        matches!(
                            entry.kind,
                            AgentLogKind::Created { .. }
                                | AgentLogKind::ConversationOpened { .. }
                                | AgentLogKind::RuntimeGenerationRetired { .. }
                                | AgentLogKind::Usage { .. }
                                | AgentLogKind::UsageUpdated { .. }
                        )
                    })
                    .collect(),
            ));
        }
        #[cfg(not(test))]
        let _ = agent;
        Ok(None)
    }
}
