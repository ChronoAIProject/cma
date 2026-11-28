//! Domain output evidence shared by publication and invisible bootstrap.
use super::*;
use layer2_core::sandbox::{agent_output_index::AgentOutputTool, AgentLogEntry, AgentLogKind};

const MAX: u64 = 9_007_199_254_740_991;

pub(in crate::store::agent_log_segments) fn tool_key(turn: u64, call: &str) -> String {
    format!("{turn}:{}:{call}", call.len())
}

pub(in crate::store::agent_log_segments) fn call_id(entry: &AgentLogEntry) -> Option<&str> {
    match &entry.kind {
        AgentLogKind::ToolCallStarted { call_id, .. }
        | AgentLogKind::ToolCallUpdated { call_id, .. }
        | AgentLogKind::ToolOutputReported { call_id, .. } => Some(call_id),
        _ => None,
    }
}

pub(in crate::store::agent_log_segments) fn tool_valid(
    tool: &AgentToolOutputState,
    through: u64,
) -> bool {
    through <= MAX
        && tool.turn_sequence > 0
        && tool.start_sequence > tool.turn_sequence
        && tool.start_sequence <= through
        && tool.item_ordinal < tool.start_sequence - tool.turn_sequence
        && tool
            .output_sequence
            .is_none_or(|s| s > tool.start_sequence && s <= through)
        && tool
            .terminal_sequence
            .is_none_or(|s| s >= tool.start_sequence && s <= through)
}

fn change_valid(change: &AgentOutputChange) -> bool {
    let c = change;
    if c.turn_sequence == 0
        || c.source_sequence <= c.turn_sequence
        || c.before.validate(c.source_sequence - 1).is_err()
        || c.after.validate(c.source_sequence).is_err()
        || c.ordinal >= c.source_sequence - c.turn_sequence
        || c.ordinal < c.before.items
        || c.ordinal > c.before.items + c.before.tool_finishes + c.before.text.deltas
        || c.item_ordinal >= c.after.items
    {
        return false;
    }
    match (&c.tool, c.message_ordinal) {
        (None, Some(_)) => c.valid_message_change(),
        (Some(tool), None) => {
            let start = u64::from(tool.start_sequence == c.source_sequence);
            let finish = u64::from(tool.terminal_sequence == Some(c.source_sequence));
            tool_valid(tool, c.source_sequence)
                && tool.turn_sequence == c.turn_sequence
                && tool.item_ordinal == c.item_ordinal
                && start + finish > 0
                && (start == 0
                    || (tool.output_sequence.is_none() && c.item_ordinal == c.before.items))
                && (start == 1 || c.item_ordinal < c.before.items)
                && c.after.items == c.before.items + start
                && c.after.tool_starts == c.before.tool_starts + start
                && c.after.tool_finishes == c.before.tool_finishes + finish
                && c.after.messages == c.before.messages
                && c.after.nonempty_messages == c.before.nonempty_messages
                && c.after.text == c.before.text
                && (start == 0 || !c.before.text.is_open())
        }
        _ => false,
    }
}

/// Return false only for a non-output record with no numeric seek fields.
pub(in crate::store::agent_log_segments) fn validate(
    e: &Evidence,
    through: u64,
) -> Result<bool, SandboxStoreError> {
    let (kind, key, valid) = match &e.record {
        EvidenceRecord::TurnOutput { index } => (
            "turn_output",
            index.turn_sequence.to_string(),
            index.validate().is_ok()
                && index.agent_id.as_str() == e.agent_id
                && index.through_sequence <= through,
        ),
        EvidenceRecord::OutputChange { change } => (
            "output_change",
            format!("{}:{}", change.turn_sequence, change.ordinal),
            change_valid(change)
                && change.source_sequence <= through
                && e.turn_sequence == i64::try_from(change.turn_sequence).ok()
                && e.change_ordinal == i64::try_from(change.ordinal).ok(),
        ),
        EvidenceRecord::ToolOutput { tool } => (
            "tool_output",
            tool_key(tool.turn_sequence, &tool.call_id),
            tool_valid(tool, through),
        ),
        _ => {
            return if e.turn_sequence.is_none() && e.change_ordinal.is_none() {
                Ok(false)
            } else {
                Err(corrupt())
            };
        }
    };
    if !valid
        || e.kind != kind
        || e.entity_key != key
        || (kind != "output_change" && (e.turn_sequence.is_some() || e.change_ordinal.is_some()))
    {
        return Err(corrupt());
    }
    Ok(true)
}

pub(in crate::store::agent_log_segments) fn document(
    head: &Head,
    record: EvidenceRecord,
) -> Result<Document, SandboxStoreError> {
    let (kind, key, turn_sequence, change_ordinal) = match &record {
        EvidenceRecord::TurnOutput { index } => {
            ("turn_output", index.turn_sequence.to_string(), None, None)
        }
        EvidenceRecord::OutputChange { change } => (
            "output_change",
            format!("{}:{}", change.turn_sequence, change.ordinal),
            Some(i64::try_from(change.turn_sequence).map_err(|_| corrupt())?),
            Some(i64::try_from(change.ordinal).map_err(|_| corrupt())?),
        ),
        EvidenceRecord::ToolOutput { tool } => (
            "tool_output",
            tool_key(tool.turn_sequence, &tool.call_id),
            None,
            None,
        ),
        _ => return Err(corrupt()),
    };
    let evidence = Evidence {
        id: identity(head, kind, &key),
        format_version: 1,
        agent_id: head.agent_id.clone(),
        incarnation: head.incarnation.clone(),
        evidence_generation: head.evidence_generation.clone(),
        kind: kind.into(),
        entity_key: key,
        through_seq: head.last_seq,
        head_revision: head.revision,
        record,
        turn_sequence,
        change_ordinal,
    };
    validate(
        &evidence,
        u64::try_from(head.last_seq).map_err(|_| corrupt())?,
    )?;
    let row = bson::serialize_to_document(&evidence).map_err(|_| corrupt())?;
    if bson::serialize_to_vec(&row).map_err(|_| corrupt())?.len() > 256 * 1024 {
        return Err(SandboxStoreError::Unavailable(
            "Agent output evidence exceeds its record budget".into(),
        ));
    }
    Ok(row)
}

/// Every row advances the current turn, even if it emits no output change.
/// A new turn ends the previous index at its immediately preceding sequence.
pub(in crate::store::agent_log_segments) fn fold(
    head: &Head,
    index: &mut Option<AgentTurnOutputIndex>,
    entry: &AgentLogEntry,
    tool: AgentOutputTool<'_>,
) -> Result<Vec<Document>, SandboxStoreError> {
    if matches!(entry.kind, AgentLogKind::TurnStarted { .. }) {
        *index = Some(AgentTurnOutputIndex::start(entry).map_err(SandboxStoreError::Corrupt)?);
        return Ok(vec![document(
            head,
            EvidenceRecord::TurnOutput {
                index: index.clone().ok_or_else(corrupt)?,
            },
        )?]);
    }
    let Some(index) = index else {
        return Ok(vec![]);
    };
    let delta = index
        .observe(entry, tool)
        .map_err(SandboxStoreError::Corrupt)?;
    let mut rows = vec![document(
        head,
        EvidenceRecord::TurnOutput {
            index: index.clone(),
        },
    )?];
    if let Some(change) = delta.change {
        rows.push(document(head, EvidenceRecord::OutputChange { change })?);
    }
    if let Some(tool) = delta.tool {
        rows.push(document(head, EvidenceRecord::ToolOutput { tool })?);
    }
    Ok(rows)
}

#[cfg(test)]
#[path = "projection_output_tests.rs"]
mod tests;
