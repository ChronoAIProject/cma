use crate::sandbox::{Sandbox, SandboxEventKind, SandboxWorkStep};

use super::{SandboxLifecycleAgentScope, SandboxLifecycleMutation, SandboxStoreError};

/// Reconstruct the exact model transition; no Agent or live writer authority is changed.
pub(super) fn validate_background_mutation(
    current: &Sandbox,
    mutation: &SandboxLifecycleMutation,
) -> Result<(), SandboxStoreError> {
    if !mutation.agents.is_empty()
        || mutation.agent_scope != SandboxLifecycleAgentScope::Touched
        || mutation.events
            != [SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }]
    {
        return Err(SandboxStoreError::Conflict);
    }
    let next = &mutation.sandbox;
    let at = &mutation.appended_at;
    let mut expected = current.clone();
    let result = match (
        current.lifecycle.background_checkpoint.as_deref(),
        next.lifecycle.background_checkpoint.as_deref(),
    ) {
        (None, Some(capture)) => {
            expected.begin_background_checkpoint(capture.plan.clone(), at.clone())
        }
        (Some(capture), None) => {
            let lease = capture
                .work
                .lease
                .as_ref()
                .ok_or(SandboxStoreError::Conflict)?;
            expected.complete_background_checkpoint_cleanup(&lease.holder, lease.fence, at.clone())
        }
        (Some(capture), Some(submitted)) => {
            if let Some(lease) = submitted.work.lease.as_ref() {
                expected.claim_background_checkpoint(
                    lease.holder.clone(),
                    at.clone(),
                    lease.expires_at.clone(),
                )
            } else {
                let lease = capture
                    .work
                    .lease
                    .as_ref()
                    .ok_or(SandboxStoreError::Conflict)?;
                expected.advance_background_checkpoint(
                    submitted.progress.clone(),
                    &lease.holder,
                    lease.fence,
                    at.clone(),
                    submitted.work.ready_at.clone(),
                )
            }
        }
        (None, None) => return Err(SandboxStoreError::Conflict),
    };
    if result.is_err() || expected != *next {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}
