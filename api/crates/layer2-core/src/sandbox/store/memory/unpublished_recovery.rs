use super::*;
use crate::sandbox::{
    apply_unpublished_runtime_recovery_with_policy, SandboxUnpublishedRecoveryRequest,
    SandboxUnpublishedRecoverySnapshot,
};
use layer3_adapter::recovery_diagnostics::{Observation, Reason, Stage};

pub(super) fn snapshot(
    state: &State,
    id: &SandboxId,
) -> Result<SandboxUnpublishedRecoverySnapshot, SandboxStoreError> {
    load(state, id, true)
}

pub(super) fn retained_snapshot(
    state: &State,
    id: &SandboxId,
) -> Result<crate::sandbox::RetainedRecoverySnapshot, SandboxStoreError> {
    let snapshot = load(state, id, false)?;
    super::maintenance::validate_event_stream(state, id)?;
    Ok(crate::sandbox::RetainedRecoverySnapshot {
        sandbox: snapshot.sandbox,
        agents: snapshot.agents,
        workspaces: snapshot.workspaces,
    })
}

fn load(
    state: &State,
    id: &SandboxId,
    require_unpublished: bool,
) -> Result<SandboxUnpublishedRecoverySnapshot, SandboxStoreError> {
    let sandbox = state
        .sandboxes
        .get(id)
        .cloned()
        .ok_or(SandboxStoreError::NotFound)?;
    let mut agents = state
        .agents
        .values()
        .filter(|agent| agent.sandbox_id == *id)
        .cloned()
        .collect::<Vec<_>>();
    let mut workspaces = state
        .workspaces
        .values()
        .filter(|workspace| workspace.sandbox_id == *id)
        .cloned()
        .collect::<Vec<_>>();
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    workspaces.sort_by(|a, b| a.id.cmp(&b.id));
    let generation = sandbox
        .unpublished_recovery_binding()
        .and_then(|binding| binding.provision_generation);
    if require_unpublished && generation.is_none() {
        return Err(SandboxStoreError::Conflict);
    }
    for agent in &agents {
        let logs = state
            .agent_logs
            .get(&agent.id)
            .map_or(&[][..], Vec::as_slice);
        for (index, entry) in logs.iter().enumerate() {
            if require_unpublished
                && matches!(entry.kind,
                AgentLogKind::ConversationOpened { runtime_generation, .. }
                | AgentLogKind::RuntimeGenerationRetired { runtime_generation, .. }
                    if generation.is_some_and(|generation| runtime_generation >= generation))
            {
                Observation::new(Stage::PublicationLog).finish(Reason::PublishedGeneration);
                return Err(SandboxStoreError::InvalidTransaction(
                    "runtime generation was already published",
                ));
            }
            if entry.agent_id != agent.id
                || entry.seq != u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1)
            {
                return Err(SandboxStoreError::SequenceConflict);
            }
        }
        if logs.last().map_or(0, |entry| entry.seq) != agent.conversation.last_seq {
            return Err(SandboxStoreError::SequenceConflict);
        }
    }
    Ok(SandboxUnpublishedRecoverySnapshot {
        sandbox,
        agents,
        workspaces,
    })
}

pub(super) fn admit(
    state: &mut State,
    request: SandboxUnpublishedRecoveryRequest,
    rotation_recovery: bool,
) -> Result<Sandbox, SandboxStoreError> {
    let current = snapshot(state, &request.snapshot.sandbox.id)?;
    let mutation =
        apply_unpublished_runtime_recovery_with_policy(&current, &request, rotation_recovery)?;
    super::maintenance::validate_event_stream(state, &current.sandbox.id)?;
    let mut candidate = state.clone();
    let sandbox = mutation.sandbox;
    candidate
        .sandboxes
        .insert(sandbox.id.clone(), sandbox.clone());
    append_sandbox_event(
        &mut candidate,
        &sandbox.id,
        request.requested_at,
        mutation.event,
    )?;
    *state = candidate;
    Ok(sandbox)
}
