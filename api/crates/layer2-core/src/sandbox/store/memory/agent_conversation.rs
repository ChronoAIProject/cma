use super::*;
use crate::sandbox::store::{
    validate_agent_conversation_admission, AgentConversationMutation, AgentConversationStore,
};
use crate::sandbox::PendingAgentApprovals;
use crate::seconds_from_rfc3339;

#[cfg(any(test, feature = "test-support"))]
impl InMemorySandboxStore {
    pub fn seed_turn_policy_fixture(&self, agent: Agent) {
        self.lock().agents.insert(agent.id.clone(), agent);
    }
}

#[async_trait]
impl AgentConversationStore for InMemorySandboxStore {
    async fn commit_turn_timeout(
        &self,
        settlement: &crate::sandbox::turn_timeout::TurnTimeoutSettlement,
    ) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        let id = &settlement.snapshot().agent.id;
        let command_id = &settlement.command().receipt.envelope.command_id;
        let actual = state.agents.get(id).ok_or(SandboxStoreError::NotFound)?;
        let command = state
            .agent_commands
            .get(command_id)
            .ok_or(SandboxStoreError::NotFound)?;
        if actual
            .conversation
            .turn_timeout
            .as_ref()
            .is_some_and(|timeout| {
                timeout.blocks_dispatch()
                    && timeout.action_id() == settlement.checkpoint().update_id
            })
            && command.status == crate::sandbox::agent_io::AgentResponseStatus::Failed
        {
            return Ok(());
        }
        if command != settlement.command() {
            return Err(SandboxStoreError::Conflict);
        }
        let mut candidate = state.clone();
        let mut lane = candidate
            .agent_command_orders
            .get(id)
            .cloned()
            .ok_or(SandboxStoreError::Conflict)?;
        let command = settlement
            .settle_command(&mut lane)
            .map_err(|_| SandboxStoreError::Conflict)?;
        commit_message(
            &mut candidate,
            settlement.snapshot().agent.clone(),
            settlement.snapshot().checkpoint.clone(),
            settlement.checkpoint().clone(),
            Some(settlement.mutation().clone()),
            None,
        )?;
        candidate.agent_commands.insert(command_id.clone(), command);
        candidate.agent_command_orders.insert(id.clone(), lane);
        *state = candidate;
        Ok(())
    }

    async fn agent_message_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<Option<super::AgentMessageSnapshot>, SandboxStoreError> {
        let state = self.lock();
        let Some(agent) = state.agents.get(id).cloned() else {
            return Ok(None);
        };
        let checkpoint = state.agent_message_checkpoints.get(id).cloned();
        if let Some(checkpoint) = &checkpoint {
            checkpoint.validate()?;
        }
        Ok(Some(super::AgentMessageSnapshot { agent, checkpoint }))
    }

    async fn agent_message_checkpoint(
        &self,
        id: &AgentId,
    ) -> Result<Option<super::AgentMessageCheckpoint>, SandboxStoreError> {
        let checkpoint = self.lock().agent_message_checkpoints.get(id).cloned();
        if let Some(checkpoint) = &checkpoint {
            checkpoint.validate()?;
        }
        Ok(checkpoint)
    }

    async fn commit_agent_reopen_update(
        &self,
        current: Agent,
        previous: Option<super::AgentMessageCheckpoint>,
        next: super::AgentMessageCheckpoint,
        mutation: Option<AgentConversationMutation>,
        guard: crate::sandbox::store::AgentReopenGuard,
    ) -> Result<crate::sandbox::AgentUpdateOutcome, SandboxStoreError> {
        if let Some(mutation) = &mutation {
            crate::sandbox::turn_timeout::validate_ordinary_publication(&current, mutation)?;
        }
        commit_message(
            &mut self.lock(),
            current,
            previous,
            next,
            mutation,
            Some(&guard),
        )
    }

    async fn commit_agent_message_update(
        &self,
        current: Agent,
        previous: Option<super::AgentMessageCheckpoint>,
        next: super::AgentMessageCheckpoint,
        mutation: Option<AgentConversationMutation>,
    ) -> Result<crate::sandbox::AgentUpdateOutcome, SandboxStoreError> {
        if let Some(mutation) = &mutation {
            crate::sandbox::turn_timeout::validate_ordinary_publication(&current, mutation)?;
        }
        commit_message(&mut self.lock(), current, previous, next, mutation, None)
    }

    async fn commit_agent_conversation(
        &self,
        mutation: AgentConversationMutation,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        let mut state = self.lock();
        let current = state
            .agents
            .get(&mutation.agent.id)
            .ok_or(SandboxStoreError::NotFound)?;
        if current.work.step.fences_conversation_output() {
            return Err(SandboxStoreError::Conflict);
        }
        crate::sandbox::turn_timeout::validate_ordinary_publication(current, &mutation)?;
        let mut candidate = state.clone();
        let appended = apply_conversation(&mut candidate, mutation)?;
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
        let mut pending = PendingAgentApprovals::default();
        for entry in state.agent_logs.get(id).into_iter().flatten() {
            pending.observe(entry);
        }
        Ok(pending.into_entries())
    }
}

fn apply_conversation(
    candidate: &mut State,
    mutation: AgentConversationMutation,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    if mutation.agent_logs.len() > MAX_AGENT_LOG_BATCH_SIZE {
        return Err(SandboxStoreError::AgentLogCapacityExceeded);
    }
    validate_mutation(candidate, &mutation)?;
    if mutation.is_managed_context_admission()
        && candidate.agent_commands.values().any(|command| {
            command.receipt.envelope.agent_id == mutation.agent.id && !command.status.is_terminal()
        })
    {
        return Err(SandboxStoreError::Conflict);
    }
    if let Some(fence) = &mutation.settings_fence {
        let snapshot = super::resource_access::snapshot(candidate, fence.target())
            .map_err(|_| SandboxStoreError::Conflict)?;
        fence
            .validate_snapshot(&snapshot)
            .map_err(|_| SandboxStoreError::Conflict)?;
    }
    super::agent_command_work::bind_dispatch(candidate, &mutation)?;
    if mutation.admission_guard.is_some() {
        candidate
            .sandboxes
            .get_mut(&mutation.agent.sandbox_id)
            .ok_or(SandboxStoreError::Conflict)?
            .cancel_recovery_pause(&mutation.appended_at)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    }

    let id = mutation.agent.id.clone();
    let checkpoint = candidate
        .agent_message_checkpoints
        .get(&id)
        .and_then(|checkpoint| {
            candidate
                .agents
                .get(&id)
                .and_then(|current| checkpoint.advance(current, &mutation.agent))
        });
    if let Some(checkpoint) = checkpoint {
        candidate
            .agent_message_checkpoints
            .insert(id.clone(), checkpoint);
    } else {
        candidate.agent_message_checkpoints.remove(&id);
    }
    candidate.agents.insert(id.clone(), mutation.agent);
    let appended = append_log_batch(
        candidate,
        &id,
        mutation.expected_last_seq,
        mutation.appended_at,
        mutation.agent_logs,
    )?;
    if let Some(fence) = &mutation.execution_fence {
        fence
            .require_current()
            .map_err(|_| SandboxStoreError::Conflict)?;
    }
    if let Some(fence) = &mutation.settings_fence {
        fence
            .require_current()
            .map_err(|_| SandboxStoreError::Conflict)?;
    }
    super::suggestions::enqueue(candidate, &id);
    Ok(appended)
}

fn validate_mutation(
    state: &State,
    mutation: &AgentConversationMutation,
) -> Result<(), SandboxStoreError> {
    let current = state
        .agents
        .get(&mutation.agent.id)
        .ok_or(SandboxStoreError::NotFound)?;
    validate_agent_conversation_admission(
        current,
        state.sandboxes.get(&current.sandbox_id),
        mutation,
    )?;
    let actual_last_seq = state
        .agent_logs
        .get(&mutation.agent.id)
        .and_then(|entries| entries.last())
        .map_or(0, |entry| entry.seq);
    if actual_last_seq != current.conversation.last_seq {
        return Err(SandboxStoreError::SequenceConflict);
    }
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

    if seconds_from_rfc3339(&mutation.appended_at).is_none()
        || mutation.runtime_generation == 0
        || mutation.agent_logs.is_empty()
        || !current.status.is_live()
        || !mutation.agent.status.is_live()
        || current.revision != mutation.expected_agent_revision
        || mutation.agent.revision != next_revision
        || current.conversation.last_seq != mutation.expected_last_seq
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
        || current.capabilities != next.capabilities
        || (current.work != next.work
            && !crate::sandbox::agent_live_skills::is_install_admission(current, next))
        || current.failure != next.failure
        || current.title != next.title
        || current.created_at != next.created_at
        || current.ready_at != next.ready_at
        || current.stopped_at != next.stopped_at
        || current.requested_stop_reason != next.requested_stop_reason
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

fn commit_message(
    state: &mut State,
    current: Agent,
    previous: Option<super::AgentMessageCheckpoint>,
    next: super::AgentMessageCheckpoint,
    mutation: Option<AgentConversationMutation>,
    guard: Option<&crate::sandbox::store::AgentReopenGuard>,
) -> Result<crate::sandbox::AgentUpdateOutcome, SandboxStoreError> {
    let project = |agent: &Agent| match guard {
        Some(guard) => guard.project(agent),
        None => Ok(agent.clone()),
    };
    next.validate()?;

    let actual = state.agent_message_checkpoints.get(&current.id);
    if let Some(guard) = guard {
        let parent = state
            .sandboxes
            .get(&current.sandbox_id)
            .ok_or(SandboxStoreError::Conflict)?;
        let actual_agent = state
            .agents
            .get(&current.id)
            .ok_or(SandboxStoreError::NotFound)?;
        if actual_agent != &current {
            return Err(SandboxStoreError::Conflict);
        }
        guard
            .validate(parent, actual_agent, &crate::now_rfc3339())
            .map_err(|_| {
                SandboxStoreError::InvalidTransaction("reopen output authority is stale")
            })?;
    } else if state
        .agents
        .get(&current.id)
        .is_some_and(|agent| agent.work.step.fences_conversation_output())
        || actual.is_some_and(|checkpoint| {
            Some(checkpoint.runtime_generation) > current.conversation.runtime_generation
        })
    {
        return Err(SandboxStoreError::Conflict);
    }
    let reanchor = mutation.is_none()
        && previous
            .as_ref()
            .and_then(|value| value.reanchored(&current))
            .as_ref()
            == Some(&next);
    if let Some(recorded) = actual.filter(|value| {
        value.update_id == next.update_id && (!reanchor || value.after_seq >= next.after_seq)
    }) {
        return if recorded.update_fingerprint == next.update_fingerprint {
            Ok(recorded.outcome.clone())
        } else {
            Err(SandboxStoreError::Conflict)
        };
    }
    if state.agents.get(&current.id) != Some(&current)
        || actual != previous.as_ref()
        || !current.status.is_live()
    {
        return Err(SandboxStoreError::Conflict);
    }
    let mut candidate = state.clone();
    if reanchor {
        candidate
            .agent_message_checkpoints
            .insert(current.id, next.clone());
        *state = candidate;
        return Ok(next.outcome);
    }
    match mutation {
        Some(mutation) => {
            if mutation.agent.id != current.id || !next.matches(&project(&mutation.agent)?) {
                return Err(SandboxStoreError::Conflict);
            }
            let appended = apply_conversation(&mut candidate, mutation)?;
            if appended != next.outcome.appended {
                return Err(SandboxStoreError::Conflict);
            }
        }
        None => {
            if !next.matches(&project(&current)?)
                || !next.outcome.appended.is_empty()
                || next.outcome.turn_closed
            {
                return Err(SandboxStoreError::Conflict);
            }
            let actual_last_seq = candidate
                .agent_logs
                .get(&current.id)
                .and_then(|logs| logs.last())
                .map_or(0, |entry| entry.seq);
            if actual_last_seq != current.conversation.last_seq {
                return Err(SandboxStoreError::SequenceConflict);
            }
        }
    }
    candidate
        .agent_message_checkpoints
        .insert(current.id.clone(), next.clone());
    *state = candidate;
    Ok(next.outcome)
}
