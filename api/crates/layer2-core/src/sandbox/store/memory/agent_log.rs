//! In-memory history reads and append operations.
use super::*;

#[async_trait]
impl AgentLogStore for InMemorySandboxStore {
    async fn append_agent_log(
        &self,
        agent_id: &AgentId,
        appended_at: String,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        let mut state = self.lock();
        if !state.agents.contains_key(agent_id) {
            return Err(SandboxStoreError::NotFound);
        }
        let log = state.agent_logs.entry(agent_id.clone()).or_default();
        let seq = u64::try_from(log.len())
            .map_err(|_| SandboxStoreError::SequenceConflict)?
            .checked_add(1)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        let event = AgentLogEntry {
            agent_id: agent_id.clone(),
            seq,
            appended_at,
            kind,
        };
        log.push(event.clone());
        suggestions::enqueue(&mut state, agent_id);
        Ok(event)
    }

    async fn agent_log(
        &self,
        agent_id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        let rows = bounded_events(
            self.lock().agent_logs.get(agent_id),
            after,
            limit,
            |event| event.seq,
        )?;
        #[cfg(test)]
        self.log_rows_read
            .fetch_add(rows.len(), std::sync::atomic::Ordering::SeqCst);
        Ok(rows)
    }

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
        let state = self.lock();
        let agent = state
            .agents
            .get(agent_id)
            .ok_or(SandboxStoreError::NotFound)?;
        let log = state
            .agent_logs
            .get(agent_id)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        if u64::try_from(log.len()).ok() != Some(agent.conversation.last_seq)
            || log.iter().enumerate().any(|(offset, entry)| {
                entry.agent_id != *agent_id || entry.seq != offset as u64 + 1
            })
            || through > agent.conversation.last_seq
        {
            return Err(SandboxStoreError::SequenceConflict);
        }
        let expected = (through - after).min(u64::from(limit)) as usize;
        Ok(log
            .iter()
            .skip(after as usize)
            .take(expected)
            .cloned()
            .collect())
    }
}
