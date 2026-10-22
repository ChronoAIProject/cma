use super::{
    AgentEventHead, AgentEventKind, AgentEventPage, AgentPageCursor, ResponseBinding, UiTransport,
    cursor::{identifier, valid_incarnation},
};
use crate::{Error, Result};

pub(super) fn head(value: AgentEventHead, agent_id: &str) -> Result<AgentEventHead> {
    if value.schema_version != 1
        || value.agent_id != agent_id
        || !valid_incarnation(&value.incarnation)
        || value.through_cursor.incarnation() != value.incarnation
    {
        return Err(Error::protocol(
            "Agent head changed identity or incarnation",
        ));
    }
    if let Some(turn) = &value.active_turn {
        if turn.cursor.incarnation() != value.incarnation
            || turn.cursor.sequence() != turn.sequence
            || turn.sequence > value.through_cursor.sequence()
            || !matches!(turn.kind, Some(AgentEventKind::TurnStarted { .. }))
        {
            return Err(Error::protocol(
                "Agent head does not identify a current turn within its boundary",
            ));
        }
        payload(
            turn.sequence,
            &turn.appended_at,
            turn.kind.as_ref(),
            turn.response_binding.as_ref(),
        )?;
    }
    Ok(value)
}

pub(super) fn eligibility(value: UiTransport) -> Result<UiTransport> {
    if value.schema_version != 1
        || value.snapshot_schema != 1
        || value.event_schema != 1
        || value.control_schema != 1
        || value.selection != "new_tabs"
        || value.input != "responses_v2"
        || value.snapshot != "/api/v2/agents/{agent_id}/events/snapshot"
        || value.events != "/api/v2/agents/{agent_id}/events"
        || value.controls != "/api/v2/agents/{agent_id}/responses/{response_id}/controls"
        || value.command_status != "/api/v2/commands/{command_id}"
    {
        return Err(Error::protocol(
            "unsupported UI transport eligibility contract",
        ));
    }
    Ok(value)
}
pub(super) fn page(
    value: AgentEventPage,
    agent_id: &str,
    limit: u16,
    previous: Option<&AgentPageCursor>,
) -> Result<AgentEventPage> {
    let through = value.through_cursor.sequence();
    let (prior_head, after) = previous.map_or((through, 0), AgentPageCursor::coordinates);
    if value.schema_version != 1
        || value.agent_id != agent_id
        || !valid_incarnation(&value.incarnation)
        || value.through_cursor.incarnation() != value.incarnation
        || prior_head != through
        || previous.is_some_and(|cursor| cursor.incarnation() != value.incarnation)
        || value.items.len() > usize::from(limit)
    {
        return Err(Error::protocol(
            "Agent snapshot changed identity, watermark, or page bound",
        ));
    }
    let mut sequence = after;
    for item in &value.items {
        sequence = sequence
            .checked_add(1)
            .ok_or_else(|| Error::protocol("Agent sequence overflow"))?;
        if item.sequence != sequence
            || item.cursor.sequence() != sequence
            || item.cursor.incarnation() != value.incarnation
        {
            return Err(Error::protocol(
                "Agent snapshot is not a consecutive immutable prefix",
            ));
        }
        payload(
            item.sequence,
            &item.appended_at,
            item.kind.as_ref(),
            item.response_binding.as_ref(),
        )?;
    }
    if sequence > through
        || value.next_page_cursor.is_none() != (sequence == through)
        || (sequence < through && value.items.is_empty())
    {
        return Err(Error::protocol(
            "Agent snapshot skipped or truncated its fixed prefix",
        ));
    }
    if let Some(next) = &value.next_page_cursor
        && (next.incarnation() != value.incarnation || next.coordinates() != (through, sequence))
    {
        return Err(Error::protocol(
            "Agent continuation disagrees with its delivered page",
        ));
    }
    Ok(value)
}
pub(super) fn payload(
    sequence: u64,
    appended_at: &str,
    kind: Option<&AgentEventKind>,
    binding: Option<&ResponseBinding>,
) -> Result<()> {
    if sequence == 0 || identifier(appended_at).is_err() {
        return Err(Error::protocol("invalid Agent event sequence or timestamp"));
    }
    use AgentEventKind::*;
    let turn = match kind {
        Some(
            TurnStarted { turn_sequence }
            | TurnMetadata { turn_sequence, .. }
            | TurnCompleted { turn_sequence }
            | TurnFailed { turn_sequence, .. }
            | TurnCancelled { turn_sequence },
        ) => Some(*turn_sequence),
        _ => None,
    };
    if turn.is_some_and(|turn| turn == 0 || turn > sequence)
        || matches!(kind, Some(TurnStarted { turn_sequence }) if *turn_sequence != sequence)
        || matches!(
            kind,
            Some(DeliveryChanged {
                operation_sequence: 0,
                ..
            })
        )
    {
        return Err(Error::protocol(
            "Agent event refers to an invalid bound turn or operation",
        ));
    }
    if let Some(binding) = binding
        && (identifier(&binding.command_id).is_err()
            || identifier(&binding.response_id).is_err()
            || !matches!(kind, Some(TurnStarted { turn_sequence }) if *turn_sequence == binding.turn_sequence))
    {
        return Err(Error::protocol(
            "Agent response binding does not identify the exact turn start",
        ));
    }
    Ok(())
}
