//! Response-specific reads retain the stream's nonrenewing release authority.
use super::*;
use crate::sandbox::{store::AgentResponseLogWindow, AgentLogKind};

impl AgentReadService {
    pub fn with_response_logs(
        self: &Arc<Self>,
        response_logs: Option<Arc<dyn super::super::store::AgentResponseLogStore>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            sessions: self.sessions.clone(),
            resources: self.resources.clone(),
            access: self.access.clone(),
            response_logs,
        })
    }

    /// The command service supplies its already-authorized immutable turn
    /// binding. A bounded window is a complete input for the existing response
    /// projection, never a truncated substitute for complete output.
    pub async fn response_event_stream_snapshot(
        &self,
        authorization: &AgentEventStreamAuthorization,
        turn_sequence: Option<u64>,
    ) -> Result<AgentEventStreamSnapshot, AgentReadError> {
        let Some(reader) = &self.response_logs else {
            return self.event_stream_snapshot(authorization).await;
        };
        let result = authorization
            .bounded(async {
                let snapshot = self.current_event_stream_snapshot(authorization).await?;
                let agent = snapshot
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("authorized response has no Agent"))?;
                let window = reader
                    .agent_response_log_window(&agent, turn_sequence)
                    .await
                    .map_err(log_store_error)?;
                window
                    .map(|window| {
                        project_window(&agent, turn_sequence, window, authorization.resource().2)
                    })
                    .transpose()
            })
            .await;
        // Also fence dependency errors and fallback decisions before releasing
        // anything; a failed private read cannot extend expired authority.
        self.validate_event_stream(authorization).await?;
        match result? {
            Some(snapshot) => Ok(snapshot),
            None => self.event_stream_snapshot(authorization).await,
        }
    }
}

fn project_window(
    agent: &Agent,
    turn: Option<u64>,
    window: AgentResponseLogWindow,
    personal: bool,
) -> Result<AgentEventStreamSnapshot, AgentReadError> {
    if window.through != agent.conversation.last_seq || window.turn_sequence != turn {
        return Err(corrupt("response source does not match its captured head"));
    }
    match turn {
        None if !window.entries.is_empty() => {
            return Err(corrupt("unbound response unexpectedly contains a turn"));
        }
        Some(turn) => {
            if window.entries.len() > 200
                || !window.entries.first().is_some_and(|entry| {
                    entry.seq == turn
                        && matches!(&entry.kind, AgentLogKind::TurnStarted {
                        turn_sequence, model: Some(_), ..
                    } if *turn_sequence == turn)
                })
            {
                return Err(corrupt("response turn lacks its retained model fence"));
            }
            for (offset, entry) in window.entries.iter().enumerate() {
                if entry.agent_id != agent.id
                    || turn.checked_add(offset as u64) != Some(entry.seq)
                    || entry.seq > window.through
                {
                    return Err(corrupt("response source interval is not consecutive"));
                }
            }
            let last = window.entries.last().expect("turn fence is present");
            if last.seq != window.through
                && !(last.seq > turn
                    && matches!(last.kind,
                    AgentLogKind::TurnStarted { turn_sequence, .. } if turn_sequence == last.seq))
            {
                return Err(corrupt("response window has no complete turn boundary"));
            }
        }
        None => {}
    }
    Ok(AgentEventStreamSnapshot {
        id: agent.id.clone(),
        created_at: agent.created_at.clone(),
        model: agent.selected_model(),
        initial_model: agent.profile.content.model.clone(),
        runtime_generation: agent.conversation.runtime_generation,
        active_turn: agent.conversation.active_turn,
        head: window.through,
        history: window
            .entries
            .into_iter()
            .map(|entry| AgentVisibleLogEntry::from_entry(entry, personal))
            .collect(),
    })
}
