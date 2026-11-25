//! Task recency evidence for transient hierarchy reads, independent of resource maintenance.
use serde::Deserialize;

use super::{Agent, AgentLogEntry, AgentLogKind};
use crate::AgentId;

pub const TASK_ACTIVITY_TYPES: &[&str] = &[
    "owner_message",
    "agent_message",
    "turn_started",
    "turn_completed",
    "turn_failed",
    "turn_interrupted",
    "approval_requested",
    "approval_resolved",
    "tool_call_started",
    "tool_call_updated",
    "tool_output_reported",
];

#[derive(Debug, Deserialize)]
pub struct TaskActivityEvidence {
    pub agent_id: AgentId,
    pub seq: u64,
    pub appended_at: String,
    pub kind: TaskActivityKind,
}

#[derive(Debug, Deserialize)]
pub struct TaskActivityKind {
    #[serde(rename = "type")]
    pub event_type: String,
    pub turn_sequence: Option<u64>,
}

impl TaskActivityEvidence {
    pub fn from_entry(entry: &AgentLogEntry) -> Option<Self> {
        let (event_type, turn_sequence) = match &entry.kind {
            AgentLogKind::OwnerMessage { .. } => ("owner_message", None),
            AgentLogKind::AgentMessage { .. } => ("agent_message", None),
            AgentLogKind::TurnStarted { turn_sequence, .. } => {
                ("turn_started", Some(*turn_sequence))
            }
            AgentLogKind::TurnCompleted { turn_sequence } => {
                ("turn_completed", Some(*turn_sequence))
            }
            AgentLogKind::TurnFailed { turn_sequence, .. } => ("turn_failed", Some(*turn_sequence)),
            AgentLogKind::TurnInterrupted { turn_sequence } => {
                ("turn_interrupted", Some(*turn_sequence))
            }
            AgentLogKind::ApprovalRequested { .. } => ("approval_requested", None),
            AgentLogKind::ApprovalResolved { .. } => ("approval_resolved", None),
            AgentLogKind::ToolCallStarted { .. } => ("tool_call_started", None),
            AgentLogKind::ToolCallUpdated { .. } => ("tool_call_updated", None),
            AgentLogKind::ToolOutputReported { .. } => ("tool_output_reported", None),
            AgentLogKind::ConversationGap { .. } => ("conversation_gap", None),
            _ => return None,
        };
        Some(Self {
            agent_id: entry.agent_id.clone(),
            seq: entry.seq,
            appended_at: entry.appended_at.clone(),
            kind: TaskActivityKind {
                event_type: event_type.into(),
                turn_sequence,
            },
        })
    }

    pub fn valid_for(&self, agent: &Agent) -> bool {
        self.agent_id == agent.id
            && self.seq > 0
            && self.seq <= agent.conversation.last_seq
            && time::OffsetDateTime::parse(
                &self.appended_at,
                &time::format_description::well_known::Rfc3339,
            )
            .is_ok()
    }

    pub fn timestamp(
        &self,
        agent: &Agent,
        start: Option<&Self>,
        gap: Option<&Self>,
    ) -> Option<&str> {
        if !self.valid_for(agent) || !TASK_ACTIVITY_TYPES.contains(&self.kind.event_type.as_str()) {
            return None;
        }
        let since = match self.kind.event_type.as_str() {
            "turn_started" if self.kind.turn_sequence == Some(self.seq) => self.seq,
            "turn_completed" | "turn_failed" | "turn_interrupted" => {
                let start = start.filter(|start| {
                    start.valid_for(agent)
                        && start.kind.event_type == "turn_started"
                        && start.kind.turn_sequence == Some(start.seq)
                        && self.kind.turn_sequence == Some(start.seq)
                        && start.seq < self.seq
                })?;
                start.seq
            }
            "turn_started" => return None,
            _ => self.seq,
        };
        if gap.is_some_and(|gap| {
            !gap.valid_for(agent) || gap.kind.event_type != "conversation_gap" || gap.seq >= since
        }) {
            return None;
        }
        Some(&self.appended_at)
    }
}

pub fn from_log(agent: &Agent, entries: &[AgentLogEntry]) -> Option<String> {
    let latest = entries
        .iter()
        .filter(|entry| entry.seq <= agent.conversation.last_seq)
        .filter_map(TaskActivityEvidence::from_entry)
        .filter(|entry| TASK_ACTIVITY_TYPES.contains(&entry.kind.event_type.as_str()))
        .max_by_key(|entry| entry.seq)?;
    let gap = entries
        .iter()
        .filter(|entry| entry.seq <= agent.conversation.last_seq)
        .filter(|entry| matches!(entry.kind, AgentLogKind::ConversationGap { .. }))
        .max_by_key(|entry| entry.seq)
        .and_then(TaskActivityEvidence::from_entry);
    let start = entries
        .iter()
        .filter(|entry| entry.seq <= agent.conversation.last_seq)
        .filter(|entry| matches!(entry.kind, AgentLogKind::TurnStarted { .. }))
        .max_by_key(|entry| entry.seq)
        .and_then(TaskActivityEvidence::from_entry);
    latest
        .timestamp(agent, start.as_ref(), gap.as_ref())
        .map(str::to_owned)
}
