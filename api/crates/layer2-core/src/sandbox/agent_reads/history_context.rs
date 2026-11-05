use super::*;
use crate::sandbox::{agent_io::AgentEvent, AgentLogKind, AgentLogTurnLookup};

impl AgentReadService {
    /// Supply the original turn start at a history window's lower boundary.
    /// This carries identity/timing only and never replays an input command.
    pub async fn event_history_context(
        &self,
        authorization: &AgentEventStreamAuthorization,
        through: u64,
    ) -> Result<Option<AgentReplayWindow>, AgentReadError> {
        let result = authorization.bounded(async {
            let snapshot = self.current_event_stream_snapshot(authorization).await?;
            let agent = snapshot.lineage.agent.ok_or_else(||corrupt("history context has no Agent"))?;
            if through > agent.conversation.last_seq { return Err(AgentReadError::InvalidPage); }
            let (active, gap) = match self.store.agent_log_turn_at(&agent, through).await.map_err(log_store_error)? {
                AgentLogTurnLookup::Inactive => (None, None),
                AgentLogTurnLookup::Active { start, gap } => (Some(*start), gap.map(|entry| *entry)),
                AgentLogTurnLookup::Legacy => {
                    let (mut active, mut gap) = (None, None);
                    self.legacy_state(&agent, &mut |entry| {
                        if entry.seq <= through {
                            match entry.kind {
                                AgentLogKind::TurnStarted { .. } => { active = Some(entry); gap = None; },
                                AgentLogKind::TurnCompleted { .. }
                                | AgentLogKind::TurnFailed { .. }
                                | AgentLogKind::TurnInterrupted { .. } => { active = None; gap = None; },
                                AgentLogKind::ConversationGap { frames } if frames > 0 && active.is_some() && gap.is_none() => gap = Some(entry),
                                _ => {}
                            }
                        }
                        Ok(())
                    }).await?;
                    (active, gap)
                }
            };
            let Some(entry) = active else { return Ok(None); };
            if entry.agent_id != agent.id || entry.seq == 0 || entry.seq > through
                || !matches!(&entry.kind, AgentLogKind::TurnStarted { turn_sequence, .. } if *turn_sequence == entry.seq) {
                return Err(corrupt("history context has no matching original turn"));
            }
            if gap.as_ref().is_some_and(|gap| gap.agent_id != agent.id || gap.seq <= entry.seq || gap.seq > through
                || !matches!(gap.kind, AgentLogKind::ConversationGap { frames } if frames > 0)) {
                return Err(corrupt("history context has no matching original loss event"));
            }
            let mut entries = Vec::with_capacity(2);
            for entry in std::iter::once(entry).chain(gap) {
                let command_candidate = match &entry.kind {
                    AgentLogKind::TurnStarted { origin_key: Some(origin), .. } => origin.strip_prefix("command:").map(str::to_owned),
                    _ => None,
                };
                let visible = AgentVisibleLogEntry::from_entry(entry, authorization.resource().2);
                let kind = visible.kind.map(|kind| AgentEvent::from(AgentLogEntry {
                    agent_id: agent.id.clone(), seq: visible.seq, appended_at: visible.appended_at.clone(), kind,
                }).kind);
                entries.push(AgentReplayEntry {
                    sequence: visible.seq, appended_at: visible.appended_at, kind,
                    response_binding: None, message_command_id: None, command_candidate,
                    steering_operation: None,
                });
            }
            Ok(Some(AgentReplayWindow {
                header: AgentEventStreamHeader {
                    id: agent.id, created_at: agent.created_at,
                    runtime_generation: agent.conversation.runtime_generation,
                    active_turn: agent.conversation.active_turn, head: agent.conversation.last_seq,
                },
                through,
                entries,
            }))
        }).await;
        self.validate_event_stream(authorization).await?;
        result
    }
}
