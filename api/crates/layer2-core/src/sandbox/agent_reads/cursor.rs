//! Incremental, verified history. A cursor retains semantic facts, never authority.
use super::*;
use crate::sandbox::AgentLogProjectionState;

#[derive(Clone, Default)]
pub struct AgentHistoryCursor {
    identity: Option<(AgentId, String)>,
    projection: AgentLogProjectionState,
}

impl AgentHistoryCursor {
    pub fn sequence(&self) -> u64 {
        self.projection.through_seq
    }
}

impl AgentReadService {
    pub async fn event_stream_page(
        &self,
        authorization: &AgentEventStreamAuthorization,
        cursor: &mut AgentHistoryCursor,
        through: Option<u64>,
    ) -> Result<AgentEventStreamSnapshot, AgentReadError> {
        let mut next = cursor.clone();
        let result = authorization
            .bounded(async {
                let snapshot = self.current_event_stream_snapshot(authorization).await?;
                let agent = snapshot
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("authorized history has no Agent"))?;
                self.read_cursor_page(&agent, &mut next, through, authorization.resource().2)
                    .await
            })
            .await;
        self.validate_event_stream(authorization).await?;
        let page = result?;
        *cursor = next;
        Ok(page)
    }

    pub async fn owner_event_stream_page(
        &self,
        caller: &Caller,
        id: &AgentId,
        cursor: &mut AgentHistoryCursor,
        through: Option<u64>,
    ) -> Result<AgentEventStreamSnapshot, AgentReadError> {
        require(caller)?;
        let agent = self.owned_lineage(caller, id).await?.agent;
        let mut next = cursor.clone();
        let result = self
            .read_cursor_page(&agent, &mut next, through, true)
            .await;
        let current = self.owned_lineage(caller, id).await?.agent;
        if current.created_at != agent.created_at {
            return Err(AgentReadError::NotFound);
        }
        let page = result?;
        *cursor = next;
        Ok(page)
    }

    async fn read_cursor_page(
        &self,
        agent: &Agent,
        cursor: &mut AgentHistoryCursor,
        through: Option<u64>,
        personal: bool,
    ) -> Result<AgentEventStreamSnapshot, AgentReadError> {
        if agent.conversation.last_seq == 0 {
            return Err(corrupt("history has no creation record"));
        }
        let identity = (agent.id.clone(), agent.created_at.clone());
        if cursor
            .identity
            .as_ref()
            .is_some_and(|prior| prior != &identity)
        {
            return Err(AgentReadError::NotFound);
        }
        let through = through.unwrap_or(agent.conversation.last_seq);
        let after = cursor.sequence();
        if after > through || through > agent.conversation.last_seq {
            return Err(AgentReadError::InvalidPage);
        }
        let count = (through - after).min(200);
        let mut history = Vec::new();
        let mut seen = 0;
        let mut bytes = 0;
        let mut full = false;
        self.visit_observed_page(
            agent,
            after,
            through,
            count.max(1) as u16,
            true,
            &mut |entry| {
                // This port includes its proof-only predecessor.
                if after > 0 && seen == 0 {
                    seen += 1;
                    if entry.seq != after || entry.agent_id != agent.id {
                        return Err(SandboxStoreError::SequenceConflict);
                    }
                    return Ok(());
                }
                seen += 1;
                if full {
                    return Ok(());
                }
                let size = serde_json::to_vec(&entry)
                    .map_err(|e| SandboxStoreError::Corrupt(e.to_string()))?
                    .len();
                if !history.is_empty() && bytes + size > 4 * 1024 * 1024 {
                    full = true;
                    return Ok(());
                }
                cursor
                    .projection
                    .observe(agent, &entry, AgentLogProjectionState::queued_owner(&entry))
                    .map_err(SandboxStoreError::Corrupt)?;
                bytes += size;
                history.push(AgentVisibleLogEntry::from_entry(entry, personal));
                Ok(())
            },
        )
        .await
        .map_err(log_store_error)?;
        if seen != count + u64::from(after > 0) {
            return Err(corrupt("history ended before its captured head"));
        }
        if cursor.sequence() == agent.conversation.last_seq {
            cursor
                .projection
                .validate_aggregate(agent)
                .map_err(AgentReadError::Corrupt)?;
        }
        cursor.identity = Some(identity);
        Ok(AgentEventStreamSnapshot {
            source_generation: None,
            id: agent.id.clone(),
            created_at: agent.created_at.clone(),
            model: agent.selected_model(),
            initial_model: agent.profile.content.model.clone(),
            runtime_generation: agent.conversation.runtime_generation,
            active_turn: agent.conversation.active_turn,
            head: agent.conversation.last_seq,
            history,
        })
    }
}
