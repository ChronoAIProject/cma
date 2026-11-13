//! Protocol-neutral output ordering facts. Text and tool payloads remain in the
//! immutable source; tool state is supplied by the caller's indexed snapshot.
use super::{AgentLogEntry, AgentLogKind};
use crate::AgentId;
use serde::{Deserialize, Serialize};

const MAX: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOutputTotals {
    pub items: u64,
    pub messages: u64,
    pub nonempty_messages: u64,
    pub tool_starts: u64,
    pub tool_finishes: u64,
}

impl AgentOutputTotals {
    pub fn validate(&self, through: u64) -> Result<(), String> {
        if through > MAX
            || self.items > through
            || self.items
                != self
                    .messages
                    .checked_add(self.tool_starts)
                    .ok_or("output count overflow")?
            || self.nonempty_messages > self.messages
            || self.tool_finishes > self.tool_starts
        {
            return Err("output counters do not describe a valid source interval".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentToolOutputState {
    pub call_id: String,
    pub turn_sequence: u64,
    pub item_ordinal: u64,
    pub start_sequence: u64,
    #[serde(deserialize_with = "required_nullable")]
    pub output_sequence: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub terminal_sequence: Option<u64>,
}

/// None is reserved for non-tool source rows. Absent requires a successful,
/// same-snapshot indexed absence check; dependency failure is never absence.
pub enum AgentOutputTool<'a> {
    None,
    Absent,
    Stored(&'a AgentToolOutputState),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOutputChange {
    pub turn_sequence: u64,
    pub ordinal: u64,
    pub source_sequence: u64,
    pub before: AgentOutputTotals,
    pub after: AgentOutputTotals,
    pub item_ordinal: u64,
    #[serde(deserialize_with = "required_nullable")]
    pub message_ordinal: Option<u64>,
    /// Original start and output-at-first-terminal references, if this is a
    /// tool change. Later output updates must not rewrite this immutable row.
    #[serde(deserialize_with = "required_nullable")]
    pub tool: Option<AgentToolOutputState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTurnOutputIndex {
    pub agent_id: AgentId,
    pub turn_sequence: u64,
    pub through_sequence: u64,
    pub changes: u64,
    pub totals: AgentOutputTotals,
}

pub struct AgentOutputDelta {
    pub change: Option<AgentOutputChange>,
    pub tool: Option<AgentToolOutputState>,
}

impl AgentTurnOutputIndex {
    pub fn start(entry: &AgentLogEntry) -> Result<Self, String> {
        if entry.seq == 0
            || entry.seq > MAX
            || !matches!(entry.kind,
            AgentLogKind::TurnStarted {turn_sequence, ..} if turn_sequence == entry.seq)
        {
            return Err("output index must begin at an original turn start".into());
        }
        Ok(Self {
            agent_id: entry.agent_id.clone(),
            turn_sequence: entry.seq,
            through_sequence: entry.seq,
            changes: 0,
            totals: AgentOutputTotals::default(),
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        self.totals.validate(self.through_sequence)?;
        if self.turn_sequence == 0
            || self.turn_sequence > self.through_sequence
            || self.changes > self.through_sequence - self.turn_sequence
            || self.totals.items > self.changes
            || self.changes
                > self
                    .totals
                    .items
                    .checked_add(self.totals.tool_finishes)
                    .ok_or("output count overflow")?
        {
            return Err("output index does not cover its turn interval".into());
        }
        Ok(())
    }

    /// Constant-size state and at most one immutable change + one tool upsert
    /// per source row. The caller publishes these with the source/head/receipt.
    pub fn observe(
        &mut self,
        entry: &AgentLogEntry,
        tool: AgentOutputTool<'_>,
    ) -> Result<AgentOutputDelta, String> {
        self.validate()?;
        if entry.agent_id != self.agent_id
            || self.through_sequence.checked_add(1) != Some(entry.seq)
            || entry.seq > MAX
            || matches!(entry.kind, AgentLogKind::TurnStarted { .. })
        {
            return Err("output index source is not the next row in this turn".into());
        }
        let mut next = self.clone();
        let before = self.totals;
        let mut item_ordinal = before.items;
        let mut message_ordinal = None;
        let mut updated_tool = None;
        let call_id = match &entry.kind {
            AgentLogKind::ToolCallStarted { call_id, .. }
            | AgentLogKind::ToolCallUpdated { call_id, .. }
            | AgentLogKind::ToolOutputReported { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        };
        let previous = match (call_id, tool) {
            (None, AgentOutputTool::None) | (Some(_), AgentOutputTool::Absent) => None,
            (Some(id), AgentOutputTool::Stored(tool)) => {
                if tool.call_id != id
                    || tool.turn_sequence != self.turn_sequence
                    || tool.item_ordinal >= before.items
                    || tool.start_sequence <= self.turn_sequence
                    || tool.start_sequence > self.through_sequence
                    || tool.output_sequence.is_some_and(|seq| {
                        seq <= tool.start_sequence || seq > self.through_sequence
                    })
                    || tool
                        .terminal_sequence
                        .is_some_and(|seq| seq < tool.start_sequence || seq > self.through_sequence)
                {
                    return Err("indexed tool state does not match its source interval".into());
                }
                Some(tool)
            }
            _ => return Err("output tool lookup has no certified presence or absence".into()),
        };
        match &entry.kind {
            AgentLogKind::AgentMessage { content } => {
                message_ordinal = Some(before.messages);
                next.totals.items += 1;
                next.totals.messages += 1;
                if !content.is_empty() {
                    next.totals.nonempty_messages += 1;
                }
            }
            AgentLogKind::ToolCallStarted {
                call_id, status, ..
            } if previous.is_none() => {
                let terminal = terminal(status).then_some(entry.seq);
                next.totals.items += 1;
                next.totals.tool_starts += 1;
                if terminal.is_some() {
                    next.totals.tool_finishes += 1;
                }
                updated_tool = Some(AgentToolOutputState {
                    call_id: call_id.clone(),
                    turn_sequence: self.turn_sequence,
                    item_ordinal,
                    start_sequence: entry.seq,
                    output_sequence: None,
                    terminal_sequence: terminal,
                });
            }
            AgentLogKind::ToolCallUpdated { status, output, .. } if previous.is_some() => {
                let mut value = previous.expect("guarded presence").clone();
                item_ordinal = value.item_ordinal;
                if output.is_some() {
                    value.output_sequence = Some(entry.seq);
                }
                if value.terminal_sequence.is_none() && terminal(status) {
                    value.terminal_sequence = Some(entry.seq);
                    next.totals.tool_finishes += 1;
                }
                updated_tool = Some(value);
            }
            AgentLogKind::ToolOutputReported { .. } if previous.is_some() => {
                let mut value = previous.expect("guarded presence").clone();
                value.output_sequence = Some(entry.seq);
                updated_tool = Some(value);
            }
            _ => {}
        }
        let change = (before != next.totals).then(|| AgentOutputChange {
            turn_sequence: self.turn_sequence,
            ordinal: self.changes,
            source_sequence: entry.seq,
            before,
            after: next.totals,
            item_ordinal,
            message_ordinal,
            tool: updated_tool.clone(),
        });
        if change.is_some() {
            next.changes += 1;
        }
        next.through_sequence = entry.seq;
        next.validate()?;
        *self = next;
        Ok(AgentOutputDelta {
            change,
            tool: updated_tool,
        })
    }
}

fn terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed")
}
fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer)
}

#[cfg(test)]
mod tests;
