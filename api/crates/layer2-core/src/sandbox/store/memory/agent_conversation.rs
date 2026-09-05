use std::collections::BTreeMap;

use super::*;
use crate::sandbox::store::{AgentConversationMutation, AgentConversationStore};

#[async_trait]
impl AgentConversationStore for InMemorySandboxStore {
    async fn commit_agent_conversation(
        &self,
        mutation: AgentConversationMutation,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        let mut state = self.lock();
        let mut candidate = state.clone();
        validate_mutation(&candidate, &mutation)?;

        let id = mutation.agent.id.clone();
        candidate.agents.insert(id.clone(), mutation.agent);
        let appended = append_log_batch(
            &mut candidate,
            &id,
            mutation.expected_last_seq,
            mutation.appended_at,
            mutation.agent_logs,
        )?;
        *state = candidate;
        Ok(appended)
    }

    async fn pending_agent_approvals(
        &self,
        id: &AgentId,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        let state = self.lock();
        if !state.agents.contains_key(id) {
            return Err(SandboxStoreError::NotFound);
        }
        let mut pending = BTreeMap::new();
        for entry in state.agent_logs.get(id).into_iter().flatten() {
            match &entry.kind {
                AgentLogKind::TurnStarted { .. } | AgentLogKind::TurnCompleted { .. } => {
                    pending.clear();
                }
                AgentLogKind::ApprovalRequested { approval_id, .. } => {
                    pending.insert(approval_id.clone(), entry.clone());
                }
                AgentLogKind::ApprovalResolved { approval_id, .. } => {
                    pending.remove(approval_id);
                }
                AgentLogKind::TurnFailed { .. }
                | AgentLogKind::TurnInterrupted { .. }
                | AgentLogKind::Stopped { .. }
                | AgentLogKind::Deleted => pending.clear(),
                _ => {}
            }
        }
        Ok(pending.into_values().collect())
    }
}

fn validate_mutation(
    state: &State,
    mutation: &AgentConversationMutation,
) -> Result<(), SandboxStoreError> {
    let current = state
        .agents
        .get(&mutation.agent.id)
        .ok_or(SandboxStoreError::NotFound)?;
    let actual_last_seq = state
        .agent_logs
        .get(&mutation.agent.id)
        .and_then(|entries| entries.last())
        .map_or(0, |entry| entry.seq);
    let count = u64::try_from(mutation.agent_logs.len())
        .map_err(|_| SandboxStoreError::SequenceConflict)?;
    let next_revision = mutation
        .expected_agent_revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    let next_last_seq = mutation
        .expected_last_seq
        .checked_add(count)
        .ok_or(SandboxStoreError::SequenceConflict)?;

    if mutation.runtime_generation == 0
        || mutation.agent_logs.is_empty()
        || current.revision != mutation.expected_agent_revision
        || mutation.agent.revision != next_revision
        || current.conversation.last_seq != mutation.expected_last_seq
        || actual_last_seq != mutation.expected_last_seq
        || mutation.agent.conversation.last_seq != next_last_seq
        || current.conversation.conversation_id != mutation.agent.conversation.conversation_id
        || current.conversation.runtime_generation != Some(mutation.runtime_generation)
        || mutation.agent.conversation.runtime_generation != Some(mutation.runtime_generation)
        || immutable_fields_changed(current, &mutation.agent)
        || mutation.agent.validate_conversation().is_err()
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

fn immutable_fields_changed(current: &Agent, next: &Agent) -> bool {
    current.id != next.id
        || current.worktree_id != next.worktree_id
        || current.workspace_id != next.workspace_id
        || current.sandbox_id != next.sandbox_id
        || current.owner != next.owner
        || current.creation_key != next.creation_key
        || current.creation_fingerprint != next.creation_fingerprint
        || current.profile != next.profile
        || current.work != next.work
        || current.failure != next.failure
        || current.title != next.title
        || current.created_at != next.created_at
        || current.ready_at != next.ready_at
        || current.stopped_at != next.stopped_at
        || current.stop_reason != next.stop_reason
        || current.deleted_at != next.deleted_at
        || current.origin != next.origin
}

fn append_log_batch(
    state: &mut State,
    id: &AgentId,
    expected_last_seq: u64,
    appended_at: String,
    kinds: Vec<AgentLogKind>,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let log = state.agent_logs.entry(id.clone()).or_default();
    if log.last().map_or(0, |entry| entry.seq) != expected_last_seq {
        return Err(SandboxStoreError::SequenceConflict);
    }
    let mut appended = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let seq = log.last().map_or(Ok(1), |entry| {
            entry
                .seq
                .checked_add(1)
                .ok_or(SandboxStoreError::SequenceConflict)
        })?;
        let entry = AgentLogEntry {
            agent_id: id.clone(),
            seq,
            appended_at: appended_at.clone(),
            kind,
        };
        log.push(entry.clone());
        appended.push(entry);
    }
    Ok(appended)
}
