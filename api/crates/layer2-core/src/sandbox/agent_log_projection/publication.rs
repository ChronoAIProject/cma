//! Whole-publication semantic planning, independent of database transport.
use super::*;

impl AgentLogProjectionState {
    /// Plan one bounded publication against a certified source state. The store
    /// separately proves source/session identity, gate authority and original
    /// queued inputs. This method grants none of those capabilities.
    ///
    /// Validate the complete batch before returning any changes. An incomplete
    /// intermediate turn is allowed only when later rows in the same publication
    /// finish its required facts. The source state is never mutated on failure.
    pub fn plan_publication(
        &self,
        current: &Agent,
        next: &Agent,
        entries: &[AgentLogEntry],
    ) -> Result<(Self, Vec<AgentLogEvidenceChange>), String> {
        self.validate_aggregate(current)?;
        if entries.len() > 64
            || current.id != next.id
            || current.owner != next.owner
            || current.sandbox_id != next.sandbox_id
            || current.workspace_id != next.workspace_id
            || current.created_at != next.created_at
            || current.creation_key != next.creation_key
            || current.creation_fingerprint != next.creation_fingerprint
            || current.creation_worktree_id() != next.creation_worktree_id()
            || next.revision < current.revision
            || current
                .conversation
                .last_seq
                .checked_add(entries.len() as u64)
                != Some(next.conversation.last_seq)
            || entries.first().is_some_and(|first| {
                entries
                    .iter()
                    .any(|entry| entry.appended_at != first.appended_at)
            })
        {
            return Err("Agent publication differs from its original source or batch".into());
        }
        let mut planned = self.clone();
        let mut changes = Vec::new();
        for entry in entries {
            changes.extend(planned.observe(next, entry, Self::queued_owner(entry))?);
        }
        planned.validate_aggregate(next)?;
        Ok((planned, changes))
    }
}

#[cfg(test)]
#[path = "publication_tests.rs"]
mod tests;
