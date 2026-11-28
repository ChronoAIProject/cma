use super::*;

use crate::sandbox::store::{
    apply_sandbox_runtime_loss, validate_sandbox_runtime_loss_request, SandboxReconciliationStore,
    SandboxRuntimeLossApplication, SandboxRuntimeLossOutcome, SandboxRuntimeLossRequest,
};

#[async_trait]
impl SandboxReconciliationStore for InMemorySandboxStore {
    async fn sandbox_runtime_authority(
        &self,
        request: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        let values = self
            .lock()
            .sandboxes
            .iter()
            .map(|(key, sandbox)| {
                if key != &sandbox.id {
                    return Err(SandboxStoreError::Corrupt(
                        "stored Sandbox key disagrees with its identity".into(),
                    ));
                }
                sandbox.validate().map_err(|error| {
                    SandboxStoreError::Corrupt(format!("stored Sandbox is invalid: {error}"))
                })?;
                Ok((key.to_string(), sandbox.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        page(values, request)
    }

    async fn recover_runtime_loss(
        &self,
        request: SandboxRuntimeLossRequest,
    ) -> Result<SandboxRuntimeLossOutcome, SandboxStoreError> {
        validate_sandbox_runtime_loss_request(&request)?;
        let mut state = self.lock();
        let current = state
            .sandboxes
            .get(&request.sandbox_id)
            .ok_or(SandboxStoreError::NotFound)?;
        let mut agents = state
            .agents
            .iter()
            .filter(|(_, agent)| agent.sandbox_id == current.id)
            .map(|(key, agent)| {
                if key != &agent.id {
                    return Err(SandboxStoreError::Corrupt(
                        "stored Agent key disagrees with its identity".into(),
                    ));
                }
                let logs = state.agent_logs.get(key).map_or(&[][..], Vec::as_slice);
                validate_log_stream(key, logs)?;
                Ok((agent.clone(), logs.last().map_or(0, |entry| entry.seq)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        agents.sort_by(|(left, _), (right, _)| left.id.cmp(&right.id));
        let mutation = match apply_sandbox_runtime_loss(current, &agents, &request)? {
            SandboxRuntimeLossApplication::Complete(outcome) => return Ok(outcome),
            SandboxRuntimeLossApplication::Persist(mutation) => mutation,
        };
        super::maintenance::validate_event_stream(&state, &current.id)?;
        let mut candidate = state.clone();
        for change in mutation.agents {
            let id = change.agent.id.clone();
            save_record!(
                candidate,
                agents,
                id.clone(),
                change.expected_revision,
                change.agent
            )?;
            let log = candidate.agent_logs.entry(id.clone()).or_default();
            let mut sequence = change.expected_last_seq;
            for kind in change.logs {
                sequence = sequence
                    .checked_add(1)
                    .ok_or(SandboxStoreError::SequenceConflict)?;
                log.push(AgentLogEntry {
                    agent_id: id.clone(),
                    seq: sequence,
                    appended_at: request.observed_at.clone(),
                    kind,
                });
            }
        }
        let id = mutation.sandbox.id.clone();
        save_record!(
            candidate,
            sandboxes,
            id.clone(),
            mutation.expected_revision,
            mutation.sandbox
        )?;
        for event in mutation.events {
            append_sandbox_event(&mut candidate, &id, request.observed_at.clone(), event)?;
        }
        *state = candidate;
        Ok(mutation.outcome)
    }
}

fn validate_log_stream(id: &AgentId, logs: &[AgentLogEntry]) -> Result<(), SandboxStoreError> {
    for (index, entry) in logs.iter().enumerate() {
        if entry.agent_id != *id {
            return Err(SandboxStoreError::Corrupt(
                "stored Agent log disagrees with its Agent".into(),
            ));
        }
        let expected = u64::try_from(index)
            .map_err(|_| SandboxStoreError::SequenceConflict)?
            .checked_add(1)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        if entry.seq != expected {
            return Err(SandboxStoreError::SequenceConflict);
        }
    }
    Ok(())
}
