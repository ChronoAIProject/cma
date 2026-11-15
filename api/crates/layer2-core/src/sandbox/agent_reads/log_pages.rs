use super::*;

impl AgentReadService {
    pub async fn log(
        &self,
        caller: &Caller,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, AgentReadError> {
        self.log_page(caller, id, after, limit)
            .await
            .map(|(entries, _)| entries)
    }

    pub async fn log_page(
        &self,
        caller: &Caller,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<(Vec<AgentLogEntry>, Option<u64>), AgentReadError> {
        require(caller)?;
        let lineage = self.owned_lineage(caller, id).await?;
        validate_limit(limit)?;
        let entries = self.read_log_page(&lineage.agent, after, limit).await?;
        let current = self.owned_lineage(caller, id).await?;
        if current.agent.created_at != lineage.agent.created_at {
            return Err(AgentReadError::NotFound);
        }
        let next = next_cursor(&entries, lineage.agent.conversation.last_seq, limit);
        Ok((entries, next))
    }

    pub(super) async fn read_log_page(
        &self,
        agent: &Agent,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, AgentReadError> {
        if let Some(page) = self
            .store
            .agent_log_page_projection(agent, after, limit)
            .await
            .map_err(log_store_error)?
        {
            return Ok(page);
        }
        let entries = self.complete_log(agent).await?;
        fold_agent_log(agent, &entries).map_err(AgentReadError::Corrupt)?;
        Ok(entries
            .into_iter()
            .filter(|entry| entry.seq > after)
            .take(usize::from(limit))
            .collect())
    }
}

pub(super) fn next_cursor(entries: &[AgentLogEntry], through: u64, limit: u16) -> Option<u64> {
    entries
        .last()
        .filter(|entry| entry.seq < through || entries.len() == usize::from(limit))
        .map(|entry| entry.seq)
}
