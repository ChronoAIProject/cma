use std::collections::BTreeMap;

use super::{AgentLogEntry, AgentLogKind};

/// Pending requests reconstructed from an ordered, committed Agent log.
#[derive(Default)]
pub struct PendingAgentApprovals(BTreeMap<String, AgentLogEntry>);

impl PendingAgentApprovals {
    pub fn observe(&mut self, entry: &AgentLogEntry) {
        match &entry.kind {
            AgentLogKind::ApprovalRequested { approval_id, .. } => {
                self.0.insert(approval_id.clone(), entry.clone());
            }
            AgentLogKind::ApprovalResolved { approval_id, .. } => {
                self.0.remove(approval_id);
            }
            AgentLogKind::TurnStarted { .. }
            | AgentLogKind::TurnCompleted { .. }
            | AgentLogKind::TurnFailed { .. }
            | AgentLogKind::TurnInterrupted { .. }
            | AgentLogKind::Stopped { .. }
            | AgentLogKind::Deleted => self.0.clear(),
            _ => {}
        }
    }

    pub fn into_entries(self) -> Vec<AgentLogEntry> {
        self.0.into_values().collect()
    }
}
