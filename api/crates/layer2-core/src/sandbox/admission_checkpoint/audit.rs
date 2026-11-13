//! Distinguish a known unfinished boundary from damaged retained semantics.
use super::*;
use crate::sandbox::AgentLogKind;

impl AgentLogCheckpoint {
    /// An existing trusted checkpoint must be revoked when a complete audit
    /// proves invalid retained semantics. Only recognized unfinished boundaries
    /// may remain in the legacy full-validation path without quarantine.
    pub fn validate_retained_semantics(
        agent: &Agent,
        entries: &[AgentLogEntry],
    ) -> Result<(), SandboxStoreError> {
        if !matches!(entries.first().map(|entry| &entry.kind),
            Some(AgentLogKind::Created { workspace_id, worktree_id })
                if workspace_id == &agent.workspace_id && worktree_id == agent.creation_worktree_id())
        {
            return Err(SandboxStoreError::Corrupt(
                "retained semantic history has no matching creation".into(),
            ));
        }
        let mut observed = Self::empty();
        for entry in entries {
            if entry.agent_id != agent.id || observed.through_seq.checked_add(1) != Some(entry.seq)
            {
                return Err(SandboxStoreError::SequenceConflict);
            }
            observed.apply_entry(entry)?;
            observed.through_seq = entry.seq;
        }
        observed.validate_internal()?;
        if !observed.matches_agent(agent) {
            return Err(SandboxStoreError::Corrupt(
                "retained semantic history disagrees with the Agent".into(),
            ));
        }
        if (observed.active_turn_sequence.is_some() && !observed.active_owner_message)
            || observed.steering_expected_sequence.is_some()
        {
            // These low-level legacy batches can end before the adjacent input
            // arrives. Their incomplete projection cannot authorize a shortcut.
            return Ok(());
        }
        crate::sandbox::agent_io::answer_context_for_log(agent, entries)
            .map(|_| ())
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
    }
}
