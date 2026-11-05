use super::*;
use crate::sandbox::{
    agent_io::AgentEvent,
    store::{AgentLogCurrentState, PendingAgentApprovalPage},
    AgentLogKind, PendingAgentApprovals,
};

impl AgentReadService {
    /// Current controls are fenced to one head. A continuation must restart if
    /// the outstanding approval set changed; old pages cannot authorize input.
    pub async fn event_current_state(
        &self,
        authorization: &AgentEventStreamAuthorization,
        through: u64,
        after_approval: u64,
        limit: u16,
    ) -> Result<(AgentReplayWindow, AgentReplayWindow, Option<u64>), AgentReadError> {
        let result = authorization
            .bounded(async {
                validate_limit(limit)?;
                let snapshot = self.current_event_stream_snapshot(authorization).await?;
                let agent = snapshot
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("current controls have no Agent"))?;
                if through != agent.conversation.last_seq || after_approval > through {
                    return Err(AgentReadError::Unavailable(
                        "Agent state changed; restart the current-state read".into(),
                    ));
                }
                let reading = match self
                    .store
                    .agent_log_current_state(&agent, after_approval, limit)
                    .await
                    .map_err(log_store_error)?
                {
                    Some(reading) => reading,
                    None => {
                        let (mut turn, mut gap, mut commands, mut plan, mut metadata) =
                            (None, None, None, None, None);
                        let mut pending = PendingAgentApprovals::default();
                        self.legacy_state(&agent, &mut |entry| {
                            pending.observe(&entry);
                            match &entry.kind {
                                AgentLogKind::TurnStarted { .. } => {
                                    turn = Some(entry);
                                    plan = None;
                                    gap = None;
                                }
                                AgentLogKind::TurnCompleted { .. }
                                | AgentLogKind::TurnFailed { .. }
                                | AgentLogKind::TurnInterrupted { .. }
                                | AgentLogKind::Stopped { .. }
                                | AgentLogKind::Deleted => {
                                    turn = None;
                                    plan = None;
                                    gap = None;
                                }
                                AgentLogKind::ConversationGap { frames }
                                    if turn.is_some() && gap.is_none() && *frames > 0 =>
                                {
                                    gap = Some(entry)
                                }
                                AgentLogKind::RuntimeGenerationRetired { .. } => commands = None,
                                AgentLogKind::AvailableCommandsSet { .. } => commands = Some(entry),
                                AgentLogKind::PlanSet { .. } if turn.is_some() => {
                                    plan = Some(entry)
                                }
                                AgentLogKind::RuntimeMetadataChanged { .. } => {
                                    metadata = Some(entry)
                                }
                                _ => {}
                            }
                            Ok(())
                        })
                        .await?;
                        let mut context = [turn, gap, commands, plan, metadata]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>();
                        context.sort_by_key(|entry| entry.seq);
                        AgentLogCurrentState {
                            context,
                            approvals: PendingAgentApprovalPage::from_entries(
                                pending.into_entries(),
                                after_approval,
                                limit,
                            )
                            .map_err(log_store_error)?,
                        }
                    }
                };
                let project =
                    |entries: Vec<AgentLogEntry>| -> Result<AgentReplayWindow, AgentReadError> {
                        let mut projected = Vec::with_capacity(entries.len());
                        for entry in entries {
                            if entry.agent_id != agent.id || entry.seq == 0 || entry.seq > through {
                                return Err(corrupt(
                                    "current control source is outside the capture",
                                ));
                            }
                            let command_candidate = match &entry.kind {
                                AgentLogKind::TurnStarted {
                                    origin_key: Some(origin),
                                    ..
                                } => origin.strip_prefix("command:").map(str::to_owned),
                                _ => None,
                            };
                            let visible =
                                AgentVisibleLogEntry::from_entry(entry, authorization.resource().2);
                            let kind = visible.kind.map(|kind| {
                                AgentEvent::from(AgentLogEntry {
                                    agent_id: agent.id.clone(),
                                    seq: visible.seq,
                                    appended_at: visible.appended_at.clone(),
                                    kind,
                                })
                                .kind
                            });
                            projected.push(AgentReplayEntry {
                                sequence: visible.seq,
                                appended_at: visible.appended_at,
                                kind,
                                response_binding: None,
                                message_command_id: None,
                                command_candidate,
                                steering_operation: None,
                            });
                        }
                        Ok(AgentReplayWindow {
                            header: AgentEventStreamHeader {
                                id: agent.id.clone(),
                                created_at: agent.created_at.clone(),
                                runtime_generation: agent.conversation.runtime_generation,
                                active_turn: agent.conversation.active_turn,
                                head: through,
                            },
                            through,
                            entries: projected,
                        })
                    };
                Ok((
                    project(reading.context)?,
                    project(reading.approvals.items)?,
                    reading.approvals.next_after,
                ))
            })
            .await;
        self.validate_event_stream(authorization).await?;
        result
    }
}
