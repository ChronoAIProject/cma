use super::*;
use crate::sandbox::store::{
    validate_checkpoint_mutation, validate_lifecycle_mutation, SandboxCheckpointMutation,
    SandboxLifecycleExecutionStore, SandboxLifecycleMutation,
};

#[async_trait]
impl SandboxLifecycleExecutionStore for InMemorySandboxStore {
    async fn sandbox_agents(
        &self,
        sandbox_id: &SandboxId,
        request: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        let values = self
            .lock()
            .agents
            .values()
            .filter(|agent| &agent.sandbox_id == sandbox_id)
            .map(|agent| (agent.id.to_string(), agent.clone()))
            .collect();
        page(values, request)
    }

    async fn publish_sandbox_checkpoint(
        &self,
        mutation: SandboxCheckpointMutation,
    ) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        let current = state
            .sandboxes
            .get(&mutation.guard.sandbox_id)
            .ok_or(SandboxStoreError::NotFound)?;
        let agents = if mutation.nonblocking {
            Vec::new()
        } else {
            sandbox_agent_log_rows(&state, &current.id)
        };
        for (agent, _) in agents.iter().filter(|(agent, _)| agent.status.is_live()) {
            let logs = state
                .agent_logs
                .get(&agent.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if logs.len() as u64 != agent.conversation.last_seq
                || logs.iter().enumerate().any(|(offset, entry)| {
                    entry.agent_id != agent.id || entry.seq != offset as u64 + 1
                })
            {
                return Err(SandboxStoreError::SequenceConflict);
            }
        }
        validate_checkpoint_mutation(current, &agents, &mutation)?;

        let mut candidate = state.clone();
        let checkpoint_key = checkpoint_key(&mutation.sandbox);
        if candidate.checkpoints.contains_key(&checkpoint_key) {
            return Err(SandboxStoreError::Duplicate);
        }
        candidate
            .checkpoints
            .insert(checkpoint_key, mutation.checkpoint.clone());
        let retained = &mutation.sandbox.lifecycle.checkpoints;
        candidate.checkpoints.retain(|(sandbox_id, _), checkpoint| {
            sandbox_id != &mutation.sandbox.id
                || retained.latest.as_ref() == Some(checkpoint)
                || retained.previous.as_ref() == Some(checkpoint)
        });
        save_record!(
            candidate,
            sandboxes,
            mutation.sandbox.id.clone(),
            mutation.guard.expected_revision,
            mutation.sandbox
        )?;
        append_sandbox_event(
            &mut candidate,
            &mutation.guard.sandbox_id,
            mutation.appended_at,
            SandboxEventKind::CheckpointSealed {
                object_reference: mutation.checkpoint.object_reference,
            },
        )?;
        *state = candidate;
        Ok(())
    }

    async fn commit_sandbox_lifecycle(
        &self,
        mutation: SandboxLifecycleMutation,
    ) -> Result<(), SandboxStoreError> {
        self.commit_lifecycle(mutation, None)
    }

    async fn reopen_refusal_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<Option<crate::sandbox::store::AgentMessageSnapshot>, SandboxStoreError> {
        crate::sandbox::store::AgentConversationStore::agent_message_snapshot(self, id).await
    }

    async fn park_sandbox_reopen_refusal(
        &self,
        mutation: SandboxLifecycleMutation,
        fence: crate::sandbox::store::ReopenRefusalFence,
    ) -> Result<(), SandboxStoreError> {
        self.commit_lifecycle(mutation, Some(fence))
    }
}

impl InMemorySandboxStore {
    fn commit_lifecycle(
        &self,
        mutation: SandboxLifecycleMutation,
        refusal: Option<crate::sandbox::store::ReopenRefusalFence>,
    ) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        let current = state
            .sandboxes
            .get(&mutation.guard.sandbox_id)
            .ok_or(SandboxStoreError::NotFound)?;
        if let Some(refusal) = refusal {
            let agent_matches = state.agents.get(&refusal.agent_id).is_some_and(|agent| {
                agent.sandbox_id == current.id && agent.revision == refusal.agent_revision
            });
            if refusal.owner.is_some()
                || !agent_matches
                || state.agent_message_checkpoints.get(&refusal.agent_id)
                    != refusal.checkpoint.as_deref()
            {
                return Err(SandboxStoreError::Conflict);
            }
        }
        let agents = sandbox_agent_log_rows(&state, &current.id);
        validate_lifecycle_mutation(current, &agents, &mutation)?;
        if current.lifecycle.checkpoint_capture.is_none()
            && mutation.sandbox.lifecycle.checkpoint_capture.is_some()
        {
            let rows = agents
                .iter()
                .map(|(agent, _)| agent.clone())
                .collect::<Vec<_>>();
            let workspaces = state
                .workspaces
                .values()
                .filter(|workspace| workspace.sandbox_id == current.id)
                .cloned()
                .collect::<Vec<_>>();
            super::super::checkpoint_capture::validate_checkpoint_capture_work(
                current,
                &rows,
                &workspaces,
            )?;
            let activities = super::maintenance::live_agent_activities(
                &state,
                &current.id,
                &mutation.appended_at,
            )?;
            super::super::checkpoint_capture::validate_checkpoint_capture_quiet(
                current,
                &activities,
                &mutation.appended_at,
            )?;
        }
        let mut candidate = state.clone();
        save_record!(
            candidate,
            sandboxes,
            mutation.sandbox.id.clone(),
            mutation.guard.expected_revision,
            mutation.sandbox
        )?;
        for change in mutation.agents {
            let id = change.agent.id.clone();
            if let Some(checkpoint) =
                candidate
                    .agent_message_checkpoints
                    .get(&id)
                    .and_then(|checkpoint| {
                        candidate
                            .agents
                            .get(&id)
                            .and_then(|current| checkpoint.advance(current, &change.agent))
                    })
            {
                candidate
                    .agent_message_checkpoints
                    .insert(id.clone(), checkpoint);
            }
            save_record!(
                candidate,
                agents,
                id.clone(),
                change.expected_revision,
                change.agent
            )?;
            for kind in change.logs {
                append_lifecycle_agent_log(
                    &mut candidate,
                    &id,
                    mutation.appended_at.clone(),
                    kind,
                )?;
            }
        }
        for event in mutation.events {
            append_sandbox_event(
                &mut candidate,
                &mutation.guard.sandbox_id,
                mutation.appended_at.clone(),
                event,
            )?;
        }
        *state = candidate;
        Ok(())
    }
}

fn sandbox_agent_rows(state: &State, sandbox_id: &SandboxId) -> Vec<Agent> {
    state
        .agents
        .values()
        .filter(|agent| &agent.sandbox_id == sandbox_id)
        .cloned()
        .collect()
}

fn sandbox_agent_log_rows(state: &State, sandbox_id: &SandboxId) -> Vec<(Agent, u64)> {
    sandbox_agent_rows(state, sandbox_id)
        .into_iter()
        .map(|agent| {
            let last_seq = state
                .agent_logs
                .get(&agent.id)
                .and_then(|entries| entries.last())
                .map_or(0, |entry| entry.seq);
            (agent, last_seq)
        })
        .collect()
}

fn append_lifecycle_agent_log(
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

fn checkpoint_key(sandbox: &Sandbox) -> (SandboxId, u64) {
    (sandbox.id.clone(), sandbox.revision)
}
