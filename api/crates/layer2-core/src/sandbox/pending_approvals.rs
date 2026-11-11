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
            kind if resets_pending(kind) => self.0.clear(),
            _ => {}
        }
    }

    /// A validated suffix is sufficient only if it contains a state reset.
    /// Without that boundary, requests from earlier history may still be pending.
    pub fn from_reset_suffix(entries: &[AgentLogEntry]) -> Option<Vec<AgentLogEntry>> {
        let boundary = entries
            .iter()
            .rposition(|entry| resets_pending(&entry.kind))?;
        let mut pending = Self::default();
        for entry in &entries[boundary..] {
            pending.observe(entry);
        }
        Some(pending.into_entries())
    }

    pub fn into_entries(self) -> Vec<AgentLogEntry> {
        self.0.into_values().collect()
    }
}

fn resets_pending(kind: &AgentLogKind) -> bool {
    matches!(
        kind,
        AgentLogKind::TurnStarted { .. }
            | AgentLogKind::TurnCompleted { .. }
            | AgentLogKind::TurnFailed { .. }
            | AgentLogKind::TurnInterrupted { .. }
            | AgentLogKind::Stopped { .. }
            | AgentLogKind::Deleted
    )
}

#[cfg(test)]
mod tests;
