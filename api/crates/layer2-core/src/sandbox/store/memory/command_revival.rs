use super::*;
use crate::sandbox::agent_io::CommandRevivalAuthorization;

pub(super) fn validate(
    state: &State,
    proof: &CommandRevivalAuthorization,
    candidate: &Sandbox,
    at: &str,
    event: &SandboxEventKind,
) -> Result<(), SandboxStoreError> {
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
    validate_command_revival_transition(&snapshot.lineage.sandbox, candidate, at, event)
}
