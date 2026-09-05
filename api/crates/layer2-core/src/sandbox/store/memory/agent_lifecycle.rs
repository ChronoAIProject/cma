use super::*;
use crate::sandbox::store::{AgentWorkMutation, AgentWorkerStore};

#[async_trait]
impl AgentWorkerStore for InMemorySandboxStore {
    async fn due_agent_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut ids: Vec<_> = self
            .lock()
            .agents
            .values()
            .filter(|agent| agent.work.is_due(now))
            .map(|agent| agent.id.clone())
            .collect();
        ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        ids.truncate(usize::from(limit));
        Ok(ids)
    }

    async fn claim_agent_work(
        &self,
        id: &AgentId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Agent>, SandboxStoreError> {
        let mut state = self.lock();
        let Some(current) = state.agents.get(id) else {
            return Ok(None);
        };
        if !current.work.is_due(&now) {
            return Ok(None);
        }
        let mut claimed = current.clone();
        claimed
            .work
            .claim(holder.to_string(), &now, lease_expires_at)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        claimed.revision = claimed
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        state.agents.insert(id.clone(), claimed.clone());
        Ok(Some(claimed))
    }

    async fn commit_agent_work(
        &self,
        mutation: AgentWorkMutation,
    ) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        let mut candidate = state.clone();
        validate_mutation(&candidate, &mutation)?;

        let agent_id = mutation.agent.id.clone();
        let sandbox_id = mutation.agent.sandbox_id.clone();
        save_record!(
            candidate,
            agents,
            agent_id.clone(),
            mutation.expected_agent_revision,
            mutation.agent
        )?;
        if let Some(versioned) = mutation.worktree {
            let worktree_id = versioned.worktree.id.clone();
            save_record!(
                candidate,
                worktrees,
                worktree_id,
                versioned.expected_revision,
                versioned.worktree
            )?;
        }
        for kind in mutation.agent_logs {
            append_agent_log(
                &mut candidate,
                &agent_id,
                mutation.appended_at.clone(),
                kind,
            )?;
        }
        if let Some(kind) = mutation.sandbox_event {
            append_sandbox_event(&mut candidate, &sandbox_id, mutation.appended_at, kind)?;
        }
        *state = candidate;
        Ok(())
    }
}

fn validate_mutation(state: &State, mutation: &AgentWorkMutation) -> Result<(), SandboxStoreError> {
    let current = state
        .agents
        .get(&mutation.agent.id)
        .ok_or(SandboxStoreError::NotFound)?;
    let next_revision = mutation
        .expected_agent_revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    if current.revision != mutation.expected_agent_revision
        || mutation.agent.revision != next_revision
        || !current.work.owns(&mutation.holder, mutation.fence)
        || mutation.holder.trim().is_empty()
        || mutation.fence == 0
        || immutable_agent_fields_changed(current, &mutation.agent)
        || mutation.agent.work.next_fence != current.work.next_fence
        || mutation
            .agent
            .work
            .lease
            .as_ref()
            .is_some_and(|lease| current.work.lease.as_ref() != Some(lease))
        || mutation.agent.validate_conversation().is_err()
        || !stop_intent_change_is_valid(current, &mutation.agent)
    {
        return Err(SandboxStoreError::Conflict);
    }

    if let Some(versioned) = mutation.worktree.as_ref() {
        validate_worktree(state, current, versioned)?;
    }
    Ok(())
}

fn stop_intent_change_is_valid(current: &Agent, next: &Agent) -> bool {
    if current.stop_reason.is_some() && next.stop_reason != current.stop_reason {
        return false;
    }
    match current.requested_stop_reason.as_ref() {
        Some(requested) => {
            (current.stop_reason.is_none()
                && next.status == current.status
                && next.requested_stop_reason.as_ref() == Some(requested)
                && next.stop_reason == current.stop_reason)
                || (current.status.is_live()
                    && next.status == crate::sandbox::AgentStatus::Stopped
                    && next.requested_stop_reason.is_none()
                    && next.stop_reason.as_ref() == Some(requested))
        }
        None => {
            next.requested_stop_reason.is_none()
                && (next.stop_reason == current.stop_reason
                    || (current.stop_reason.is_none()
                        && current.status.is_live()
                        && next.status == crate::sandbox::AgentStatus::Stopped
                        && next.stop_reason.is_some()))
        }
    }
}

fn immutable_agent_fields_changed(current: &Agent, next: &Agent) -> bool {
    current.id != next.id
        || current.worktree_id != next.worktree_id
        || current.workspace_id != next.workspace_id
        || current.sandbox_id != next.sandbox_id
        || current.owner != next.owner
        || current.creation_key != next.creation_key
        || current.creation_fingerprint != next.creation_fingerprint
        || current.profile != next.profile
        || current.created_at != next.created_at
        || current.origin != next.origin
}

fn validate_worktree(
    state: &State,
    agent: &Agent,
    versioned: &crate::sandbox::store::VersionedWorktree,
) -> Result<(), SandboxStoreError> {
    let current = state
        .worktrees
        .get(&versioned.worktree.id)
        .ok_or(SandboxStoreError::NotFound)?;
    let next = &versioned.worktree;
    if current.id != agent.worktree_id
        || current.revision != versioned.expected_revision
        || next.revision != versioned.expected_revision.checked_add(1).unwrap_or(0)
        || current.id != next.id
        || current.workspace_id != next.workspace_id
        || current.sandbox_id != next.sandbox_id
        || current.owner != next.owner
        || current.name != next.name
        || current.path != next.path
        || current.branch != next.branch
        || current.base_commit != next.base_commit
        || current.agent_id != next.agent_id
        || current.status == next.status
        || current.status.transition(next.status).is_err()
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

fn append_agent_log(
    state: &mut State,
    agent_id: &AgentId,
    appended_at: String,
    kind: AgentLogKind,
) -> Result<(), SandboxStoreError> {
    let log = state.agent_logs.entry(agent_id.clone()).or_default();
    let seq = u64::try_from(log.len())
        .map_err(|_| SandboxStoreError::SequenceConflict)?
        .checked_add(1)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    log.push(AgentLogEntry {
        agent_id: agent_id.clone(),
        seq,
        appended_at,
        kind,
    });
    Ok(())
}
