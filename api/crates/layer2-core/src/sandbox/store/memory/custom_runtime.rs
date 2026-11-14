use super::*;
use crate::sandbox::store::{
    apply_custom_runtime_migration, runtime_migration_now_ms, RuntimeMigrationClearance,
    SandboxCustomRuntimeMigrationAdmission,
};

pub(super) fn admit(
    store: &InMemorySandboxStore,
    mut admission: SandboxCustomRuntimeMigrationAdmission,
) -> Result<Sandbox, SandboxStoreError> {
    // Replay is read-only and cannot conflict with an already settling runtime.
    {
        let state = store.lock();
        let current = state
            .sandboxes
            .get(admission.sandbox_id())
            .ok_or(SandboxStoreError::NotFound)?;
        if admission.is_replay(current)? {
            return Ok(current.clone());
        }
    }
    let _desktop = crate::sandbox::try_acquire_runtime_migration_guard(admission.sandbox_id())
        .map_err(|_| SandboxStoreError::Conflict)?;
    let mut state = store.lock();
    let current = state
        .sandboxes
        .get(admission.sandbox_id())
        .cloned()
        .ok_or(SandboxStoreError::NotFound)?;
    if admission.is_replay(&current)? {
        return Ok(current);
    }
    admission.basis.admitted_at = crate::now_rfc3339();
    let now_ms = runtime_migration_now_ms()?;
    match &admission.basis.clearance {
        RuntimeMigrationClearance::Paused if current.status == SandboxStatus::Paused => {}
        RuntimeMigrationClearance::Observed { proof } => {
            super::runtime_activity::validate_in_memory(&state, &current.id, proof, now_ms)?;
        }
        _ => return Err(SandboxStoreError::Conflict),
    }
    let agents = super::maintenance::sandbox_agents(&state, &current.id)?;
    let busy = state.workspaces.values().any(|workspace| {
        workspace.sandbox_id == current.id
            && (workspace.work.step.is_pending() || workspace.work.lease.is_some())
    }) || state.agent_commands.values().any(|command| {
        !command.status.is_terminal()
            && agents
                .iter()
                .any(|agent| agent.id == command.receipt.envelope.agent_id)
    }) || state.sandbox_sessions.iter().any(|((id, _), expiry)| {
        id == &current.id
            && crate::seconds_from_rfc3339(expiry)
                .is_none_or(|at| i128::from(at) * 1000 > i128::from(now_ms))
    });
    if busy {
        return Err(SandboxStoreError::Conflict);
    }
    let application = apply_custom_runtime_migration(&current, &agents, &admission)?;
    super::maintenance::persist_application(&mut state, application, admission.basis.admitted_at)?;
    state
        .sandboxes
        .get(&current.id)
        .cloned()
        .ok_or(SandboxStoreError::NotFound)
}
