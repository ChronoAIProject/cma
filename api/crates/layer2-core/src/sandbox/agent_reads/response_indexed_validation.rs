use super::*;

pub(super) fn metadata(
    value: &AgentOutputMetadata,
    agent: &Agent,
    turn: Option<u64>,
) -> Result<(), AgentReadError> {
    if value.agent_id != agent.id
        || value.through != agent.conversation.last_seq
        || value.created_at != agent.created_at
        || value.initial_model != agent.profile.content.model
        || value.runtime_generation != agent.conversation.runtime_generation
        || value.active_turn != agent.conversation.active_turn
        || value.turn.as_ref().map(|t| t.facts.turn_sequence) != turn
        || value.index.as_ref().map(|i| i.turn_sequence) != turn
    {
        return Err(corrupt(
            "indexed output does not match its authorized capture",
        ));
    }
    if let (Some(turn), Some(index)) = (&value.turn, &value.index) {
        index
            .validate()
            .map_err(|_| corrupt("invalid output interval"))?;
        turn.facts
            .validate(index.through_sequence)
            .map_err(|_| corrupt("invalid turn interval"))?;
        if index.agent_id != value.agent_id
            || index.through_sequence > value.through
            || turn.terminal_sequence != turn.facts.end_sequence
            || turn.usage_sequence != turn.facts.response_usage_sequence
            || turn.prewrite_sequence != turn.facts.prewrite_failure_sequence
            || turn.legacy_model_known != turn.facts.legacy_model_known
        {
            return Err(corrupt("output facts disagree with the certified interval"));
        }
    }
    Ok(())
}

pub(super) fn tool(
    value: &AgentToolOutputState,
    call: &str,
    index: &crate::sandbox::agent_output_index::AgentTurnOutputIndex,
) -> Result<(), AgentReadError> {
    if value.call_id != call
        || value.turn_sequence != index.turn_sequence
        || value.item_ordinal >= index.totals.items
        || value.start_sequence <= index.turn_sequence
        || value.start_sequence > index.through_sequence
        || value
            .output_sequence
            .is_some_and(|s| s <= value.start_sequence || s > index.through_sequence)
        || value
            .terminal_sequence
            .is_some_and(|s| s < value.start_sequence || s > index.through_sequence)
    {
        return Err(corrupt(
            "tool references leave their certified output interval",
        ));
    }
    Ok(())
}

pub(super) fn change(
    value: &AgentOutputChange,
    index: &crate::sandbox::agent_output_index::AgentTurnOutputIndex,
) -> Result<(), AgentReadError> {
    value
        .before
        .validate(index.through_sequence)
        .map_err(|_| corrupt("invalid prior output counters"))?;
    value
        .after
        .validate(index.through_sequence)
        .map_err(|_| corrupt("invalid next output counters"))?;
    if value.source_sequence <= index.turn_sequence
        || value.source_sequence > index.through_sequence
    {
        return Err(corrupt("output source leaves its turn interval"));
    }
    let mut expected = value.before;
    match (&value.tool, value.message_ordinal) {
        (None, Some(_)) if value.valid_message_change() => {
            return Ok(());
        }
        (Some(t), None) => {
            tool(t, &t.call_id, index)?;
            if t.item_ordinal != value.item_ordinal
                || t.start_sequence > value.source_sequence
                || t.output_sequence.is_some_and(|s| s > value.source_sequence)
                || t.terminal_sequence
                    .is_some_and(|s| s > value.source_sequence)
            {
                return Err(corrupt(
                    "immutable tool references are ahead of their change",
                ));
            }
            if t.start_sequence == value.source_sequence {
                if value.item_ordinal != expected.items || expected.text.is_open() {
                    return Err(corrupt("invalid tool item ordinal"));
                }
                expected.items += 1;
                expected.tool_starts += 1;
                expected.tool_finishes += u64::from(t.terminal_sequence.is_some());
            } else if t.terminal_sequence == Some(value.source_sequence) {
                expected.tool_finishes += 1;
            } else {
                return Err(corrupt(
                    "output change is neither a start nor first terminal",
                ));
            }
        }
        _ => return Err(corrupt("output change has incompatible item references")),
    }
    if expected != value.after {
        return Err(corrupt("output change counters disagree with its item"));
    }
    Ok(())
}

pub(super) fn point(
    metadata: &AgentOutputMetadata,
    row: &AgentLogEntry,
) -> Result<(), AgentReadError> {
    use crate::sandbox::{
        AgentLogKind, AgentOperationDelivery, AgentOperationKind, AgentTurnOutcome,
    };
    let Some(turn) = &metadata.turn else {
        return Ok(());
    };
    let facts = &turn.facts;
    let sequence = facts.turn_sequence;
    if row.seq == sequence
        && (!matches!(&row.kind,AgentLogKind::TurnStarted {turn_sequence,model,origin_key}
        if *turn_sequence==sequence && model==&facts.model && origin_key==&facts.origin_key)
            || row.appended_at != facts.started_at)
    {
        return Err(corrupt("turn start differs from its certified facts"));
    }
    if turn.usage_sequence == Some(row.seq) {
        let tokens = match &row.kind {
            AgentLogKind::Usage {
                input_tokens,
                output_tokens,
                ..
            } => Some((*input_tokens, *output_tokens)),
            AgentLogKind::UsageUpdated {
                turn_sequence: Some(observed),
                turn_input_tokens,
                turn_output_tokens,
                ..
            } if *observed == sequence => Some((*turn_input_tokens, *turn_output_tokens)),
            _ => None,
        };
        if tokens
            != facts
                .response_usage
                .map(|v| (v.input_tokens, v.output_tokens))
        {
            return Err(corrupt("usage source differs from its certified facts"));
        }
    }
    if facts.metadata_sequence == Some(row.seq)
        && !matches!(&row.kind,AgentLogKind::TurnMetadataSet {turn_sequence,..} if *turn_sequence==sequence)
    {
        return Err(corrupt("metadata source belongs to another turn"));
    }
    let ended_at = match &facts.outcome {
        AgentTurnOutcome::Active => None,
        AgentTurnOutcome::Completed { at }
        | AgentTurnOutcome::Failed { at, .. }
        | AgentTurnOutcome::Interrupted { at } => Some(at),
    };
    if turn.terminal_sequence == Some(row.seq) {
        let matches = match (&row.kind, &facts.outcome) {
            (AgentLogKind::TurnCompleted { turn_sequence }, _) => {
                *turn_sequence == sequence && facts.source_completed
            }
            (
                AgentLogKind::TurnFailed {
                    turn_sequence,
                    failure,
                },
                AgentTurnOutcome::Failed {
                    failure: expected, ..
                },
            ) => *turn_sequence == sequence && failure == expected && !facts.source_completed,
            (
                AgentLogKind::TurnInterrupted { turn_sequence },
                AgentTurnOutcome::Interrupted { .. },
            ) => *turn_sequence == sequence && !facts.source_completed,
            _ => false,
        };
        if !matches || ended_at != Some(&row.appended_at) {
            return Err(corrupt("terminal source differs from its certified facts"));
        }
    }
    if turn.prewrite_sequence == Some(row.seq)
        && (!matches!(&row.kind,AgentLogKind::OperationDeliveryChanged {
        operation_sequence,operation:AgentOperationKind::SendMessage,state:AgentOperationDelivery::PreWriteFailed
    } if operation_sequence.checked_add(1)==Some(sequence))
            || ended_at != Some(&row.appended_at))
    {
        return Err(corrupt(
            "prewrite failure is not adjacent to its original terminal",
        ));
    }
    Ok(())
}
