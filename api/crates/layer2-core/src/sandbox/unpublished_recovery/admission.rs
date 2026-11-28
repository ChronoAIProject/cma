use super::{
    refused, SandboxEventKind, SandboxStoreError, SandboxUnpublishedRecoveryMutation,
    SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoverySnapshot,
};

/// Store implementations must read/fence the complete child sets and compare the
/// current snapshot with the verified one in the same transaction as this write.
pub fn apply_unpublished_runtime_recovery(
    current: &SandboxUnpublishedRecoverySnapshot,
    request: &SandboxUnpublishedRecoveryRequest,
) -> Result<SandboxUnpublishedRecoveryMutation, SandboxStoreError> {
    apply_unpublished_runtime_recovery_with_policy(current, request, false)
}

pub fn apply_unpublished_runtime_recovery_with_policy(
    current: &SandboxUnpublishedRecoverySnapshot,
    request: &SandboxUnpublishedRecoveryRequest,
    rotation_recovery: bool,
) -> Result<SandboxUnpublishedRecoveryMutation, SandboxStoreError> {
    if current != &request.snapshot {
        return Err(SandboxStoreError::Conflict);
    }
    request.proof.validate_for(current)?;
    let advances_managed_release = rotation_recovery
        && current.sandbox.can_advance_unpublished_reopen()
        && current
            .sandbox
            .managed_runtime_lane()
            .zip(request.released_runtime.as_ref())
            .and_then(|(lane, release)| lane.version_in(release))
            == Some(request.target_runtime_version.as_str());
    if request.automatic
        && (current.sandbox.legacy_unpublished_allocation()
            || current.sandbox.lifecycle.unpublished_recovery_attempted
            || current.sandbox.lifecycle.unpublished_recovery.is_some()
            || (!advances_managed_release
                && current.sandbox.automatic_unpublished_recovery_target()
                    != Some(request.target_runtime_version.as_str())))
    {
        return Err(refused());
    }
    let now = crate::seconds_from_rfc3339(&request.requested_at).ok_or_else(refused)?;
    if crate::seconds_from_rfc3339(&current.sandbox.updated_at).is_none_or(|at| at > now)
        || request.target_runtime_version.trim().is_empty()
        || request.target_runtime_version.len() > 512
        || request.target_runtime_version.chars().any(char::is_control)
    {
        return Err(refused());
    }
    let sandbox = current
        .sandbox
        .begin_unpublished_runtime_recovery(
            &current.agents,
            &request.proof,
            &request.target_runtime_version,
            request.automatic,
            &request.requested_at,
        )
        .map_err(|_| refused())?;
    Ok(SandboxUnpublishedRecoveryMutation {
        sandbox,
        event: SandboxEventKind::RuntimeRecoveryAdmitted {
            previous_status: current.sandbox.status,
            target_runtime_version: request.target_runtime_version.clone(),
            proof: Box::new(request.proof.clone()),
        },
    })
}
