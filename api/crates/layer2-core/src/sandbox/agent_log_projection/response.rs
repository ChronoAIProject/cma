//! Response parity facts shared by streaming bootstrap and publication.
use super::*;

impl AgentLogProjectionState {
    pub(super) fn observe_response(
        &mut self,
        entry: &AgentLogEntry,
        changes: &mut Vec<AgentLogEvidenceChange>,
    ) {
        let previous = self.pending_prewrite_failure.take();
        if matches!(
            entry.kind,
            AgentLogKind::OperationDeliveryChanged {
                operation: AgentOperationKind::UpdateSettings,
                ..
            }
        ) {
            self.settings_changed = true;
        }
        let turn = self.active.as_mut().or(self.latest_completed.as_mut());
        let Some(turn) = turn.filter(|turn| Some(turn.turn_sequence) == self.latest_turn) else {
            return;
        };
        match &entry.kind {
            AgentLogKind::ApprovalRequested { .. } | AgentLogKind::ApprovalResolved { .. }
                if turn.end_sequence.is_none() =>
            {
                turn.approval_change_sequence = Some(entry.seq);
                changes.push(AgentLogEvidenceChange::Turn(Box::new(turn.clone())));
            }
            AgentLogKind::Usage {
                input_tokens,
                output_tokens,
                ..
            } => {
                turn.response_usage = Some(AgentTurnUsage {
                    turn_sequence: turn.turn_sequence,
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                });
                turn.response_usage_sequence = Some(entry.seq);
                changes.push(AgentLogEvidenceChange::Turn(Box::new(turn.clone())));
            }
            AgentLogKind::UsageUpdated {
                turn_sequence: Some(sequence),
                turn_input_tokens,
                turn_output_tokens,
                ..
            } if *sequence == turn.turn_sequence => {
                turn.response_usage = Some(AgentTurnUsage {
                    turn_sequence: *sequence,
                    input_tokens: *turn_input_tokens,
                    output_tokens: *turn_output_tokens,
                });
                turn.response_usage_sequence = Some(entry.seq);
                changes.push(AgentLogEvidenceChange::Turn(Box::new(turn.clone())));
            }
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence,
                operation: AgentOperationKind::SendMessage,
                state: AgentOperationDelivery::PreWriteFailed,
            } if turn.end_sequence.is_none()
                && operation_sequence.checked_add(1) == Some(turn.turn_sequence) =>
            {
                self.pending_prewrite_failure =
                    Some((*operation_sequence, entry.seq, entry.appended_at.clone()));
            }
            AgentLogKind::TurnFailed { turn_sequence, .. }
                if *turn_sequence == turn.turn_sequence =>
            {
                turn.prewrite_failure_sequence = previous
                    .filter(|(operation, sequence, at)| {
                        operation.checked_add(1) == Some(*turn_sequence)
                            && sequence.checked_add(1) == Some(entry.seq)
                            && at == &entry.appended_at
                    })
                    .map(|(_, sequence, _)| sequence);
            }
            _ => {}
        }
    }
}

impl AgentLogTurnFacts {
    pub(super) fn validate_response_facts(&self, through: u64) -> Result<(), String> {
        if self.response_usage.is_some() != self.response_usage_sequence.is_some()
            || self.approval_change_sequence.is_some_and(|seq| {
                seq <= self.turn_sequence
                    || seq > through
                    || self.end_sequence.is_some_and(|end| seq >= end)
            })
            || self
                .response_usage
                .is_some_and(|usage| usage.turn_sequence != self.turn_sequence)
            || self
                .response_usage_sequence
                .is_some_and(|seq| seq <= self.turn_sequence || seq > through)
            || self.prewrite_failure_sequence.is_some_and(|seq| {
                seq <= self.turn_sequence
                    || seq.checked_add(1) != self.end_sequence
                    || !matches!(self.outcome, AgentTurnOutcome::Failed { .. })
            })
        {
            return Err("Agent response facts are outside their original interval".into());
        }
        Ok(())
    }
}
