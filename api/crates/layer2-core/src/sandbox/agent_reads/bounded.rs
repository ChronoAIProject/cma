use super::*;

impl AgentReadService {
    pub(super) async fn bounded_log_page(
        &self,
        agent: &Agent,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, AgentReadError> {
        let segmented = self
            .store
            .agent_log_projection(agent)
            .await
            .map_err(log_store_error)?
            .is_some();
        let after = after.min(agent.conversation.last_seq);
        let mut entries = Vec::new();
        let mut bytes = 0_usize;
        let mut full = false;
        let mut visit = |entry: AgentLogEntry| {
            if full || entry.seq <= after || entries.len() == usize::from(limit) {
                return Ok(());
            }
            let size = serde_json::to_vec(&entry)
                .map_err(|_| SandboxStoreError::Corrupt("invalid history row".into()))?
                .len();
            if bytes + size > 4 * 1024 * 1024 {
                if entries.is_empty() {
                    return Err(SandboxStoreError::Corrupt(
                        "event exceeds history page budget".into(),
                    ));
                }
                full = true;
                return Ok(());
            }
            bytes += size;
            entries.push(entry);
            Ok(())
        };
        if segmented {
            self.store
                .visit_agent_log_range(
                    &agent.id,
                    after,
                    agent.conversation.last_seq,
                    limit,
                    &mut visit,
                )
                .await
                .map_err(log_store_error)?;
        } else {
            self.legacy_state(agent, &mut visit).await?;
        }
        Ok(entries)
    }

    pub(super) async fn complete_log(
        &self,
        agent: &Agent,
    ) -> Result<Vec<AgentLogEntry>, AgentReadError> {
        let through_seq = agent.conversation.last_seq;
        if through_seq == 0 {
            return Err(corrupt("agent log has no creation record"));
        }
        if through_seq > 10_128 {
            return Err(AgentReadError::PaginationRequired);
        }
        let mut entries = Vec::new();
        let mut remaining: usize = 4 * 1024 * 1024;
        let mut over_budget = false;
        let result = self
            .store
            .visit_agent_log_prefix(&agent.id, through_seq, &mut |entry| {
                let bytes = serde_json::to_vec(&entry)
                    .map_err(|_| SandboxStoreError::Corrupt("invalid history record".into()))?
                    .len();
                let Some(left) = remaining.checked_sub(bytes) else {
                    over_budget = true;
                    return Err(SandboxStoreError::InvalidPage);
                };
                remaining = left;
                entries.push(entry);
                Ok(())
            })
            .await;
        if over_budget {
            return Err(AgentReadError::PaginationRequired);
        }
        result.map_err(log_store_error)?;
        if u64::try_from(entries.len()).ok() != Some(through_seq) {
            return Err(corrupt("agent log ended before its high-water mark"));
        }
        for (offset, entry) in entries.iter().enumerate() {
            let expected = u64::try_from(offset)
                .ok()
                .and_then(|offset| offset.checked_add(1))
                .ok_or_else(|| corrupt("agent log sequence overflowed"))?;
            if entry.agent_id != agent.id || entry.seq != expected || entry.seq > through_seq {
                return Err(corrupt("agent log identity or sequence is invalid"));
            }
        }
        self.reject_uncommitted_tail(agent, through_seq).await?;
        Ok(entries)
    }

    pub(super) async fn reject_uncommitted_tail(
        &self,
        agent: &Agent,
        through_seq: u64,
    ) -> Result<(), AgentReadError> {
        if through_seq == u64::MAX {
            return Ok(());
        }
        let tail = self
            .store
            .agent_log_entry(&agent.id, through_seq.saturating_add(1))
            .await
            .map_err(log_store_error)?;
        let Some(extra) = tail else {
            return Ok(());
        };
        if extra.agent_id != agent.id || extra.seq != through_seq.saturating_add(1) {
            return Err(corrupt("agent log has an invalid tail"));
        }
        let current = self
            .store
            .get_agent(&agent.id)
            .await
            .map_err(log_store_error)?
            .ok_or_else(|| corrupt("agent disappeared while its log was read"))?;
        if current.id != agent.id
            || current.owner != agent.owner
            || current.worktree_id != agent.worktree_id
            || current.workspace_id != agent.workspace_id
            || current.sandbox_id != agent.sandbox_id
            || current.conversation.last_seq < extra.seq
        {
            return Err(corrupt("agent log extends past its aggregate"));
        }
        Ok(())
    }
}
