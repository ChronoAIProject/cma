use super::super::conversation_fold_support::{
    complete_first_message, fail_first_message, observe_first_message, observe_operation,
};
use super::super::{Agent, AgentLogEntry, AgentLogKind, AgentMessageAssembly, AgentStatus};
use super::{changed, FoldedConversationUpdate};

impl Agent {
    /// Prove one ordinary output commit using its bounded durable log interval.
    pub fn proves_output_advance(&self, after: &Agent, turn: u64, rows: &[AgentLogEntry]) -> bool {
        if self.conversation.active_turn != Some(turn)
            || self.conversation.conversation_id.is_none()
            || self.conversation.runtime_generation.is_none()
            || !self.status.is_live()
            || self
                .conversation
                .turn_timeout
                .as_ref()
                .is_some_and(|timeout| timeout.blocks_dispatch())
            || self.revision.checked_add(1) != Some(after.revision)
            || rows.is_empty()
            || rows.len() > 32
            || self.conversation.last_seq.checked_add(rows.len() as u64)
                != Some(after.conversation.last_seq)
            || rows.iter().enumerate().any(|(index, row)| {
                row.agent_id != self.id
                    || row.appended_at != after.updated_at
                    || row.seq != self.conversation.last_seq + index as u64 + 1
            })
        {
            return false;
        }

        let terminal = match rows.last().map(|row| &row.kind) {
            Some(AgentLogKind::StatusChanged {
                from,
                to: AgentStatus::Ready,
            }) if *from == self.status => {
                match rows
                    .iter()
                    .filter_map(|row| match row.kind {
                        AgentLogKind::TurnCompleted { turn_sequence } if turn_sequence == turn => {
                            Some(false)
                        }
                        AgentLogKind::TurnInterrupted { turn_sequence }
                            if turn_sequence == turn =>
                        {
                            Some(true)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .as_slice()
                {
                    [interrupted] => Some(*interrupted),
                    _ => return false,
                }
            }
            _ => None,
        };

        [false, true].into_iter().any(|prompt| {
        let mut agent = self.clone();
        let at = &after.updated_at;
        let mut logs = Vec::new();
        if matches!(rows[0].kind, AgentLogKind::AgentMessage { .. }) {
            logs.push(rows[0].kind.clone());
        }
        let result = (|| {
            if let Some(interrupted) = terminal {
                observe_operation(&mut agent, prompt, interrupted, at, &mut logs)?;
                if agent.conversation.observations.usage.is_none() {
                    if let Some(usage) = rows.get(logs.len()).filter(|row| matches!(
                        row.kind, AgentLogKind::Usage { context_used: 0, context_window: None, .. }
                    )) {
                        logs.push(usage.kind.clone());
                    }
                }
                if interrupted {
                    fail_first_message(
                        &mut agent, "agent_turn_failed", "the first-message turn was interrupted", at, &mut logs,
                    )?;
                    logs.push(AgentLogKind::TurnInterrupted { turn_sequence: turn });
                } else {
                    complete_first_message(&mut agent, at, &mut logs)?;
                    logs.push(AgentLogKind::TurnCompleted { turn_sequence: turn });
                }
                logs.push(AgentLogKind::StatusChanged { from: self.status, to: AgentStatus::Ready });
            } else {
                observe_first_message(&mut agent, prompt, at, &mut logs)?;
                observe_operation(&mut agent, prompt, false, at, &mut logs)?;
                if let Some(delta) = rows.get(logs.len()).filter(|row| matches!(
                    &row.kind, AgentLogKind::AgentMessageDelta { content, .. } if !content.is_empty()
                )) {
                    logs.push(delta.kind.clone());
                }
                if !logs.iter().any(|kind| matches!(kind, AgentLogKind::AgentMessageDelta { .. })) {
                    return Ok(false);
                }
            }
            let folded = changed(
                agent, logs, AgentMessageAssembly::default(),
                terminal.map(|_| AgentStatus::Ready),
                terminal.is_none().then_some(turn),
                terminal.is_some(), at,
            )?;
            Ok::<_, super::super::AgentConversationError>(matches!(
                folded, FoldedConversationUpdate::Changed(change)
                    if change.agent == *after && change.logs.iter().eq(rows.iter().map(|row| &row.kind))
            ))
        })();
        result.unwrap_or(false)
    })
    }
}
