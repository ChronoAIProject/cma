use super::*;
use crate::sandbox::store::{
    apply_agent_connection_recovery, apply_agent_sign_in,
    validate_agent_cleanup_capacity_log_with_retry, validate_agent_work_mutation_with_sandbox,
    AgentConnectionRecoveryApplication, AgentConnectionRecoveryCandidate,
    AgentConnectionRecoveryOutcome, AgentConnectionRecoveryRequest, AgentWorkDiscovery,
    AgentWorkMutation, AgentWorkerStore, PendingAgentWork,
};

#[async_trait]
impl AgentWorkerStore for InMemorySandboxStore {
    async fn live_agent_connections(
        &self,
        request: &PageRequest,
    ) -> Result<Page<AgentConnectionRecoveryCandidate>, SandboxStoreError> {
        let values = self
            .lock()
            .agents
            .values()
            .filter_map(AgentConnectionRecoveryCandidate::from_agent)
            .map(|candidate| (candidate.agent_id.to_string(), candidate))
            .collect();
        page(values, request)
    }

    async fn schedule_agent_connection_recovery(
        &self,
        request: AgentConnectionRecoveryRequest,
    ) -> Result<AgentConnectionRecoveryOutcome, SandboxStoreError> {
        let mut state = self.lock();
        let current = state
            .agents
            .get(&request.agent_id)
            .ok_or(SandboxStoreError::NotFound)?;
        match apply_agent_connection_recovery(current, &request)? {
            AgentConnectionRecoveryApplication::Persist(agent) => {
                state.agents.insert(agent.id.clone(), *agent);
                Ok(AgentConnectionRecoveryOutcome::Scheduled)
            }
            AgentConnectionRecoveryApplication::Complete(outcome) => Ok(outcome),
        }
    }

    async fn due_agent_work_filtered(
        &self,
        now: &str,
        limit: u16,
        filter: AgentWorkDiscovery<'_>,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut ids: Vec<_> = self
            .lock()
            .agents
            .values()
            .filter(|agent| agent.work.is_due(now))
            .filter(|agent| filter.startup.includes(agent))
            .filter(|agent| {
                !(filter.receiver_holds.contains(&agent.id)
                    && crate::sandbox::AgentWorkStep::RECEIVER_PACED.contains(&agent.work.step))
            })
            .filter(|agent| {
                !filter
                    .acquisition_cooldowns
                    .and_then(|cooldowns| cooldowns.get(&agent.id))
                    .is_some_and(|work| PendingAgentWork::from_agent(agent).as_ref() == Some(work))
            })
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
        let parent = state
            .sandboxes
            .get(&current.sandbox_id)
            .ok_or(SandboxStoreError::NotFound)?;
        if !current.work.is_due(&now)
            || parent.lifecycle.checkpoint_capture.is_some()
            || (parent.status == crate::sandbox::SandboxStatus::Rotating
                && !parent.restored_connection_is_ready(current))
        {
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
        self.commit_work(mutation, None, None)
    }

    async fn commit_agent_skill_result(
        &self,
        mutation: AgentWorkMutation,
        guard: crate::sandbox::store::AgentSkillResultGuard,
    ) -> Result<(), SandboxStoreError> {
        self.commit_work(mutation, None, Some(&guard))
    }

    async fn commit_agent_work_repeated_failure(
        &self,
        retry: crate::sandbox::store::AgentRepeatedFailure,
        _observed: Option<crate::sandbox::store::AgentRetryObservation>,
    ) -> Result<(), SandboxStoreError> {
        self.commit_work(retry.mutation, Some(&retry.marker), None)
    }
}

impl InMemorySandboxStore {
    fn commit_work(
        &self,
        mutation: AgentWorkMutation,
        marker: Option<&AgentLogEntry>,
        skill_guard: Option<&crate::sandbox::store::AgentSkillResultGuard>,
    ) -> Result<(), SandboxStoreError> {
        if mutation.agent_logs.len() > MAX_AGENT_LOG_BATCH_SIZE {
            return Err(SandboxStoreError::AgentLogCapacityExceeded);
        }
        let mut state = self.lock();
        let mut candidate = state.clone();
        validate_mutation(&candidate, &mutation, marker)?;

        let current = candidate
            .agents
            .get(&mutation.agent.id)
            .ok_or(SandboxStoreError::NotFound)?;
        let sandbox = candidate
            .sandboxes
            .get(&current.sandbox_id)
            .ok_or(SandboxStoreError::NotFound)?;
        let needs_guard =
            crate::sandbox::store::requires_agent_skill_result_guard(current, &mutation);
        if needs_guard != skill_guard.is_some() {
            return Err(SandboxStoreError::Conflict);
        }
        if let Some(guard) = skill_guard {
            guard.validate(
                current,
                sandbox,
                candidate
                    .workspaces
                    .get(&current.workspace_id)
                    .ok_or(SandboxStoreError::NotFound)?,
                candidate
                    .worktrees
                    .get(&current.worktree_id)
                    .ok_or(SandboxStoreError::NotFound)?,
            )?;
        }
        if let Some(next) = apply_agent_sign_in(sandbox, current, &mutation)? {
            candidate.sandboxes.insert(next.id.clone(), next);
        }

        let agent_id = mutation.agent.id.clone();
        let sandbox_id = mutation.agent.sandbox_id.clone();
        if let Some(checkpoint) = candidate.agent_message_checkpoints.get(&agent_id) {
            let current = candidate
                .agents
                .get(&agent_id)
                .ok_or(SandboxStoreError::NotFound)?;
            if let Some(carried) = checkpoint.advance(current, &mutation.agent) {
                candidate
                    .agent_message_checkpoints
                    .insert(agent_id.clone(), carried);
            }
        }
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

fn validate_mutation(
    state: &State,
    mutation: &AgentWorkMutation,
    marker: Option<&AgentLogEntry>,
) -> Result<(), SandboxStoreError> {
    let current = state
        .agents
        .get(&mutation.agent.id)
        .ok_or(SandboxStoreError::NotFound)?;
    validate_agent_cleanup_capacity_log_with_retry(
        current,
        mutation,
        state
            .agent_logs
            .get(&current.id)
            .and_then(|entries| entries.last()),
        marker,
    )?;
    let current_worktree = state
        .worktrees
        .get(&current.worktree_id)
        .ok_or(SandboxStoreError::NotFound)?;
    let actual_last_seq = state
        .agent_logs
        .get(&mutation.agent.id)
        .and_then(|entries| entries.last())
        .map_or(0, |entry| entry.seq);
    let sandbox = state
        .sandboxes
        .get(&current.sandbox_id)
        .ok_or(SandboxStoreError::NotFound)?;
    let workspace = state
        .workspaces
        .get(&current.workspace_id)
        .ok_or(SandboxStoreError::NotFound)?;
    crate::sandbox::store::validate_agent_workspace_binding(current, mutation, sandbox, workspace)?;
    validate_agent_work_mutation_with_sandbox(
        current,
        current_worktree,
        actual_last_seq,
        mutation,
        sandbox,
    )
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
