//! Settlement from authenticated sparse source points and complete indexed facts.
use super::*;
use crate::sandbox::{AgentLogTurnFacts, AgentTurnOutcome};

/// Internal persistence input, never client authority. The store must certify
/// the current head, complete turn/operation indexes and pending-approval query
/// in the same transaction, and authenticate every original source point.
pub struct CertifiedCommandSource<'a> {
    pub through: u64,
    pub facts: &'a AgentLogTurnFacts,
    pub start: &'a AgentLogEntry,
    pub fence: &'a AgentLogEntry,
    pub delivery: &'a AgentLogEntry,
    pub terminal: Option<&'a AgentLogEntry>,
    pub prewrite: Option<&'a AgentLogEntry>,
    pub last_approval: Option<&'a AgentLogEntry>,
    /// One authenticated member proves nonemptiness; None certifies absence.
    /// This is not a list of all approvals and cannot seed a subsequent fold.
    pub pending_approval: Option<&'a AgentLogEntry>,
}

impl CommandEvidence {
    /// Build a one-shot settlement fold without reading unrelated message text.
    /// Contiguous `observe` is unchanged; this separate entry point relies on
    /// the store's complete publication indexes to certify omitted events.
    pub fn from_certified_source(
        command: &AdmittedAgentCommand,
        source: CertifiedCommandSource<'_>,
    ) -> Result<Self, CommandLedgerError> {
        let mut evidence = Self::new(command)?;
        let dispatch = &evidence.dispatch;
        let facts = source.facts;
        facts
            .validate(source.through)
            .map_err(|_| CommandLedgerError::Corrupt)?;
        if facts.turn_sequence != dispatch.turn_sequence
            || facts.runtime_generation != dispatch.runtime_generation
            || source.start.seq != facts.turn_sequence
            || source.start.appended_at != facts.started_at
            || !matches!(&source.start.kind, AgentLogKind::TurnStarted { turn_sequence, origin_key, model }
                if *turn_sequence == facts.turn_sequence && origin_key == &facts.origin_key && model == &facts.model)
            || source.fence.seq != dispatch.operation_sequence
            || source.delivery.seq < source.fence.seq
            || !matches!(&source.delivery.kind, AgentLogKind::OperationDeliveryChanged { operation_sequence, operation, .. }
                if *operation_sequence == dispatch.operation_sequence && *operation == evidence.kind)
            || source.terminal.map(|row| row.seq) != facts.end_sequence
            || source.prewrite.map(|row| row.seq)
                != evidence
                    .message
                    .then_some(facts.prewrite_failure_sequence)
                    .flatten()
            || source.last_approval.map(|row| row.seq) != facts.approval_change_sequence
            || (evidence.message && source.fence.seq.checked_add(1) != Some(source.start.seq))
            || (!evidence.message
                && (source.fence.seq <= source.start.seq
                    || facts
                        .end_sequence
                        .is_some_and(|end| source.fence.seq >= end)))
        {
            return Err(CommandLedgerError::Corrupt);
        }
        validate_terminal(facts, source.terminal)?;
        if source.prewrite.is_some_and(|row| {
            row.kind
                != (AgentLogKind::OperationDeliveryChanged {
                    operation_sequence: dispatch.operation_sequence,
                    operation: evidence.kind,
                    state: AgentOperationDelivery::PreWriteFailed,
                })
                || source.terminal.is_none_or(|end| {
                    row.seq.checked_add(1) != Some(end.seq) || row.appended_at != end.appended_at
                })
                || row != source.delivery
        }) || source.last_approval.is_some_and(|row| {
            !matches!(
                row.kind,
                AgentLogKind::ApprovalRequested { .. } | AgentLogKind::ApprovalResolved { .. }
            )
        }) || source.pending_approval.is_some_and(|row| {
            facts.end_sequence.is_some()
                || row.seq <= facts.turn_sequence
                || facts
                    .approval_change_sequence
                    .is_none_or(|last| row.seq > last)
                || !matches!(row.kind, AgentLogKind::ApprovalRequested { .. })
        }) {
            return Err(CommandLedgerError::Corrupt);
        }

        if let Some(AgentLogEntry {
            kind: AgentLogKind::ApprovalRequested { approval_id, .. },
            seq,
            ..
        }) = source.pending_approval
        {
            let contradicts = source.last_approval.is_some_and(|last| match &last.kind {
                AgentLogKind::ApprovalResolved {
                    approval_id: resolved,
                    ..
                } => resolved == approval_id,
                AgentLogKind::ApprovalRequested {
                    approval_id: latest,
                    ..
                } => latest == approval_id && last.seq != *seq,
                _ => false,
            });
            if contradicts {
                return Err(CommandLedgerError::Corrupt);
            }
        }

        let mut rows = vec![source.start, source.fence, source.delivery];
        rows.extend(source.terminal);
        rows.extend(source.prewrite);
        rows.extend(source.last_approval);
        rows.extend(source.pending_approval);
        rows.sort_unstable_by_key(|row| row.seq);
        if rows.iter().any(|row| {
            row.seq == 0
                || row.seq > source.through
                || row.agent_id != dispatch.claim.agent_id
                || seconds_from_rfc3339(&row.appended_at).is_none()
        }) || rows
            .windows(2)
            .any(|pair| pair[0].seq == pair[1].seq && pair[0] != pair[1])
        {
            return Err(CommandLedgerError::Corrupt);
        }
        rows.dedup_by_key(|row| row.seq);
        for row in rows {
            // The authenticated indexes certify that omitted rows cannot change
            // this command's delivery/terminal facts. Keep original sequences
            // so adjacency and source-deferred checks remain exact.
            evidence.cursor = row.seq - 1;
            evidence.observe(row)?;
        }
        if !evidence.fence_seen || !evidence.turn_seen {
            return Err(CommandLedgerError::Corrupt);
        }
        if evidence.message && !evidence.status.is_terminal() {
            evidence.approvals.clear();
            if let Some(AgentLogEntry {
                kind: AgentLogKind::ApprovalRequested { approval_id, .. },
                ..
            }) = source.pending_approval
            {
                evidence.approvals.insert(approval_id.clone());
                evidence.status = AgentResponseStatus::WaitingAction;
            } else if source
                .last_approval
                .is_some_and(|row| matches!(row.kind, AgentLogKind::ApprovalRequested { .. }))
            {
                return Err(CommandLedgerError::Corrupt);
            }
        }
        evidence.cursor = source.through;
        evidence.sealed = true;
        Ok(evidence)
    }
}

fn validate_terminal(
    facts: &AgentLogTurnFacts,
    terminal: Option<&AgentLogEntry>,
) -> Result<(), CommandLedgerError> {
    let valid = match (terminal, &facts.outcome) {
        (None, AgentTurnOutcome::Active) => true,
        (Some(row), AgentTurnOutcome::Completed { at }) => {
            facts.source_completed
                && row.appended_at == *at
                && row.kind
                    == (AgentLogKind::TurnCompleted {
                        turn_sequence: facts.turn_sequence,
                    })
        }
        (Some(row), AgentTurnOutcome::Interrupted { at }) => {
            row.appended_at == *at
                && row.kind
                    == (AgentLogKind::TurnInterrupted {
                        turn_sequence: facts.turn_sequence,
                    })
        }
        (Some(row), AgentTurnOutcome::Failed { at, failure }) if !facts.source_completed => {
            row.appended_at == *at
                && row.kind
                    == (AgentLogKind::TurnFailed {
                        turn_sequence: facts.turn_sequence,
                        failure: failure.clone(),
                    })
        }
        // Response projection marks lost output as failed; command settlement
        // retains the original durable terminal, just like the contiguous fold.
        (Some(row), AgentTurnOutcome::Failed { at, .. }) => {
            facts.source_completed
                && facts.lost_frames > 0
                && row.appended_at == *at
                && row.kind
                    == (AgentLogKind::TurnCompleted {
                        turn_sequence: facts.turn_sequence,
                    })
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(CommandLedgerError::Corrupt)
    }
}
