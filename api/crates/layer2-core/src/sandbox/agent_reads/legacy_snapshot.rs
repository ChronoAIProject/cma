//! Complete compatibility reads for consumers not yet using cursor pages.
use super::*;

impl AgentReadService {
    pub async fn event_stream_snapshot(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<AgentEventStreamSnapshot, AgentReadError> {
        let result = authorization
            .bounded(self.read_event_stream_snapshot(authorization))
            .await;
        // Private dependency failures and timeouts must pass the same fence as
        // successful buffered history. No error may extend expired authority.
        self.validate_event_stream(authorization).await?;
        result
    }

    async fn read_event_stream_snapshot(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<AgentEventStreamSnapshot, AgentReadError> {
        let snapshot = self.current_event_stream_snapshot(authorization).await?;
        let agent = snapshot
            .lineage
            .agent
            .ok_or_else(|| corrupt("authorized event snapshot has no Agent"))?;
        let entries = self.complete_log(&agent).await?;
        fold_agent_log(&agent, &entries).map_err(AgentReadError::Corrupt)?;
        Ok(AgentEventStreamSnapshot {
            model: agent.selected_model(),
            id: agent.id,
            created_at: agent.created_at,
            initial_model: agent.profile.content.model,
            runtime_generation: agent.conversation.runtime_generation,
            active_turn: agent.conversation.active_turn,
            head: agent.conversation.last_seq,
            history: entries
                .into_iter()
                .map(|entry| AgentVisibleLogEntry::from_entry(entry, authorization.resource().2))
                .collect(),
        })
    }
}
