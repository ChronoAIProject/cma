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
        (None, Some(message))
            if message == expected.messages && value.item_ordinal == expected.items =>
        {
            expected.messages += 1;
            expected.items += 1;
            let nonempty = value
                .after
                .nonempty_messages
                .checked_sub(expected.nonempty_messages)
                .filter(|n| *n <= 1)
                .ok_or_else(|| corrupt("invalid message counter change"))?;
            expected.nonempty_messages += nonempty;
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
                if value.item_ordinal != expected.items {
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
