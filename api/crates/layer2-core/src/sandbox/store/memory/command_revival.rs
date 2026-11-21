use super::*;
use crate::sandbox::agent_io::CommandRevivalAuthorization;

pub(super) fn validate(
    state: &mut State,
    proof: &CommandRevivalAuthorization,
    candidate: &Sandbox,
    at: &str,
    event: &SandboxEventKind,
    managed_runtime_version: Option<&str>,
) -> Result<(), SandboxStoreError> {
    super::bindings::require_conversation_authority(state, proof.binding_authority())?;
    let command = state
        .agent_commands
        .get(&proof.command().receipt.envelope.command_id)
        .ok_or(SandboxStoreError::Conflict)?;
    proof
        .require_command(command)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let snapshot = super::resource_access::snapshot(state, &proof.target())
        .map_err(|_| SandboxStoreError::Conflict)?;
    proof
        .validate_snapshot(&snapshot)
        .map_err(|_| SandboxStoreError::Conflict)?;
    validate_command_revival_transition(
        &snapshot.lineage.sandbox,
        candidate,
        at,
        event,
        managed_runtime_version,
    )?;
    let resources = proof
        .authorization_audits()
        .map_err(|_| SandboxStoreError::Conflict)?;
    let audit = crate::sandbox::agent_io::CommandAuthorizationAudit::new(
        command,
        crate::sandbox::agent_io::CommandAuthorizationEffect::Revival {
            sandbox_revision: candidate.revision,
        },
        resources,
    )
    .map_err(|_| SandboxStoreError::Conflict)?;
    super::command_audit::append(state, audit).map_err(|_| SandboxStoreError::Conflict)
}
