use crate::sandbox::{
    Sandbox, SandboxCheckpointRef, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxPausePhase, SandboxRevivePhase, SandboxStatus,
};

pub(super) fn checkpoint_sandbox_advanced(
    current: &Sandbox,
    next: &Sandbox,
    checkpoint: &SandboxCheckpointRef,
    appended_at: &str,
    nonblocking: bool,
) -> bool {
    let mut expected = current.clone();
    let result = if nonblocking {
        let Some(capture) = current.lifecycle.background_checkpoint.as_ref() else {
            return false;
        };
        let Some(lease) = capture.work.lease.as_ref() else {
            return false;
        };
        expected.publish_background_checkpoint(
            checkpoint.clone(),
            &lease.holder,
            lease.fence,
            appended_at.into(),
        )
    } else if let Some(capture) = current.lifecycle.checkpoint_capture.as_ref() {
        let Some(lease) = capture.work.lease.as_ref() else {
            return false;
        };
        expected
            .publish_captured_checkpoint(
                checkpoint.clone(),
                &lease.holder,
                lease.fence,
                appended_at.into(),
            )
            .map(|_| ())
    } else if current.lifecycle.operation.is_none() {
        expected
            .publish_opportunistic_checkpoint(checkpoint.clone(), appended_at.into())
            .map(|_| ())
    } else {
        let Some(lease) = current.work.lease.as_ref() else {
            return false;
        };
        expected
            .publish_lifecycle_checkpoint(
                checkpoint.clone(),
                &lease.holder,
                lease.fence,
                appended_at.into(),
            )
            .map(|_| ())
    };
    result.is_ok() && expected == *next
}

pub(super) fn lifecycle_sandbox_advanced(
    current: &Sandbox,
    next: &Sandbox,
    appended_at: &str,
) -> bool {
    let Some(lease) = current.work.lease.as_ref() else {
        return false;
    };
    let mut expected = current.clone();
    let retries_same_phase = current.lifecycle.operation == next.lifecycle.operation
        && current.status == next.status
        && next.work.attempts == current.work.attempts.saturating_add(1);
    let reserves_runtime = current.lifecycle.runtime_provision.is_none()
        && next.lifecycle.runtime_provision.is_some()
        && current.lifecycle.operation == next.lifecycle.operation;
    let renewed_writer = current
        .lifecycle
        .operation
        .as_ref()
        .zip(next.lifecycle.operation.as_ref())
        .is_some_and(|(old, new)| {
            old.authority().writer_resume_generation != new.authority().writer_resume_generation
        });
    let recorded_quiescence = next
        .lifecycle
        .unpublished_recovery
        .as_ref()
        .and_then(|recovery| recovery.quiescence.clone())
        .filter(|receipt| {
            current
                .lifecycle
                .unpublished_recovery
                .as_ref()
                .is_some_and(|recovery| recovery.quiescence.as_ref() != Some(receipt))
        });
    let result = if let Some(receipt) = recorded_quiescence {
        expected.complete_unpublished_runtime_quiescence(
            receipt,
            &lease.holder,
            lease.fence,
            appended_at.into(),
        )
    } else if current.credential.current != next.credential.current {
        let Some(authority) = next.credential.current.clone() else {
            return false;
        };
        expected.replace_lifecycle_service_access_credential(
            authority,
            &lease.holder,
            lease.fence,
            appended_at.into(),
        )
    } else if renewed_writer {
        expected.retry_lifecycle_after_writer_resume(
            next.work.ready_at.clone(),
            &lease.holder,
            lease.fence,
            appended_at.into(),
        )
    } else if reserves_runtime {
        expected
            .reserve_lifecycle_runtime_generation(
                next.lifecycle.runtime_generation_high_water,
                &lease.holder,
                lease.fence,
                appended_at.into(),
            )
            .map(|_| ())
    } else if let Some(failure) = next
        .runtime_update_request()
        .and_then(|request| request.failure.clone())
        .filter(|_| {
            current
                .runtime_update_request()
                .is_some_and(|request| request.failure.is_none())
        })
    {
        expected.park_failed_runtime_update(failure, &lease.holder, lease.fence, appended_at.into())
    } else if retries_same_phase {
        expected.retry_lifecycle(
            next.work.ready_at.clone(),
            &lease.holder,
            lease.fence,
            appended_at.into(),
        )
    } else if let (SandboxStatus::Failed, None, Some(failure)) = (
        next.status,
        next.lifecycle.operation.as_ref(),
        next.failure.as_ref(),
    ) {
        expected
            .fail_lifecycle(
                failure.detail.clone(),
                &lease.holder,
                lease.fence,
                appended_at.into(),
            )
            .map(|_| ())
    } else {
        advance_phase(&mut expected, next, &lease.holder, lease.fence, appended_at)
    };
    result.is_ok() && expected == *next
}

fn advance_phase(
    expected: &mut Sandbox,
    submitted: &Sandbox,
    holder: &str,
    fence: u64,
    at: &str,
) -> Result<(), crate::sandbox::SandboxModelError> {
    match expected
        .lifecycle
        .operation
        .clone()
        .ok_or_else(invalid_execution)?
    {
        SandboxLifecycleOperation::Pause { phase, .. } => match phase {
            SandboxPausePhase::CaptureAndPublishCheckpoint => {
                if expected.lifecycle.home_recovery.is_some() {
                    expected.complete_lifecycle_persistent_pause(holder, fence, at.into())
                } else {
                    Err(invalid_execution())
                }
            }
            SandboxPausePhase::PauseAgents => {
                expected.complete_lifecycle_agent_pause(holder, fence, at.into())
            }
            SandboxPausePhase::TeardownRuntime => {
                if submitted.lifecycle.suspended_runtime.is_some() {
                    expected.complete_lifecycle_runtime_suspension(holder, fence, at.into())
                } else {
                    expected.complete_lifecycle_runtime_teardown(holder, fence, at.into())
                }
            }
            SandboxPausePhase::PublishPaused => {
                expected.publish_lifecycle_paused(holder, fence, at.into())
            }
            SandboxPausePhase::ResumeWriters => expected.complete_pause_cancellation(
                submitted.work.ready_at.clone(),
                holder,
                fence,
                at.into(),
            ),
        },
        SandboxLifecycleOperation::Revive { phase, .. } => {
            advance_revive_phase(expected, submitted, phase, holder, fence, at)
        }
        SandboxLifecycleOperation::ImageRotation { phase, .. } => {
            advance_rotation_phase(expected, submitted, phase, holder, fence, at)
        }
    }
}

fn advance_revive_phase(
    expected: &mut Sandbox,
    submitted: &Sandbox,
    phase: SandboxRevivePhase,
    holder: &str,
    fence: u64,
    at: &str,
) -> Result<(), crate::sandbox::SandboxModelError> {
    match phase {
        SandboxRevivePhase::CheckpointVerification => {
            verify_checkpoint(expected, holder, fence, at)
        }
        SandboxRevivePhase::RuntimeProvisioning => {
            bind_runtime(expected, submitted, holder, fence, at)
        }
        SandboxRevivePhase::CredentialDelivery => {
            expected.complete_lifecycle_credential_delivery(holder, fence, at.into())
        }
        SandboxRevivePhase::HomeRestore => {
            expected.complete_lifecycle_home_restore(holder, fence, at.into())
        }
        SandboxRevivePhase::ConversationReopen => {
            expected.complete_lifecycle_conversation_reopen(holder, fence, at.into())
        }
        SandboxRevivePhase::QueuedMessageDelivery => {
            advance_queue(expected, submitted, holder, fence, at)
        }
        SandboxRevivePhase::PublishActive => publish_active(expected, submitted, holder, fence, at),
    }
}

fn advance_rotation_phase(
    expected: &mut Sandbox,
    submitted: &Sandbox,
    phase: SandboxImageRotationPhase,
    holder: &str,
    fence: u64,
    at: &str,
) -> Result<(), crate::sandbox::SandboxModelError> {
    match phase {
        SandboxImageRotationPhase::CaptureAndPublishCheckpoint
            if expected.retained_home_recovery().is_some() =>
        {
            expected.complete_lifecycle_persistent_pause(holder, fence, at.into())
        }
        SandboxImageRotationPhase::CaptureAndPublishCheckpoint => Err(invalid_execution()),
        SandboxImageRotationPhase::PauseAgents => {
            expected.complete_lifecycle_agent_pause(holder, fence, at.into())
        }
        SandboxImageRotationPhase::TeardownRuntime => {
            if submitted.lifecycle.suspended_runtime.is_some() {
                expected.complete_lifecycle_runtime_suspension(holder, fence, at.into())
            } else {
                expected.complete_lifecycle_runtime_teardown(holder, fence, at.into())
            }
        }
        SandboxImageRotationPhase::CheckpointVerification => {
            verify_checkpoint(expected, holder, fence, at)
        }
        SandboxImageRotationPhase::RuntimeProvisioning => {
            bind_runtime(expected, submitted, holder, fence, at)
        }
        SandboxImageRotationPhase::CredentialDelivery => {
            expected.complete_lifecycle_credential_delivery(holder, fence, at.into())
        }
        SandboxImageRotationPhase::HomeRestore => {
            expected.complete_lifecycle_home_restore(holder, fence, at.into())
        }
        SandboxImageRotationPhase::ConversationReopen => {
            expected.complete_lifecycle_conversation_reopen(holder, fence, at.into())
        }
        SandboxImageRotationPhase::QueuedMessageDelivery => {
            advance_queue(expected, submitted, holder, fence, at)
        }
        SandboxImageRotationPhase::PublishActive => {
            publish_active(expected, submitted, holder, fence, at)
        }
    }
}

fn verify_checkpoint(
    expected: &mut Sandbox,
    holder: &str,
    fence: u64,
    at: &str,
) -> Result<(), crate::sandbox::SandboxModelError> {
    if let Some(source) = expected.retained_home_recovery().cloned() {
        return expected.complete_lifecycle_home_verification(&source, holder, fence, at.into());
    }
    let checkpoint = expected
        .lifecycle
        .checkpoints
        .latest
        .clone()
        .ok_or_else(invalid_execution)?;
    expected.complete_lifecycle_checkpoint_verification(&checkpoint, holder, fence, at.into())
}

fn bind_runtime(
    expected: &mut Sandbox,
    submitted: &Sandbox,
    holder: &str,
    fence: u64,
    at: &str,
) -> Result<(), crate::sandbox::SandboxModelError> {
    let binding = submitted.runtime.clone().ok_or_else(invalid_execution)?;
    if expected.admitted_retained_runtime_transition().is_some() {
        let evidence = submitted
            .lifecycle
            .retained_startup_policy
            .as_ref()
            .ok_or_else(invalid_execution)?;
        expected.bind_retained_lifecycle_runtime(
            binding,
            evidence.observation.clone(),
            holder,
            fence,
            at.into(),
        )
    } else {
        expected.bind_lifecycle_runtime(binding, holder, fence, at.into())
    }
}

fn advance_queue(
    expected: &mut Sandbox,
    submitted: &Sandbox,
    holder: &str,
    fence: u64,
    at: &str,
) -> Result<(), crate::sandbox::SandboxModelError> {
    if expected.lifecycle.operation == submitted.lifecycle.operation {
        expected.publish_lifecycle_queue_progress(holder, fence, at.into())
    } else {
        expected.complete_lifecycle_queued_message_delivery(holder, fence, at.into())
    }
}

fn publish_active(
    expected: &mut Sandbox,
    submitted: &Sandbox,
    holder: &str,
    fence: u64,
    at: &str,
) -> Result<(), crate::sandbox::SandboxModelError> {
    expected.publish_lifecycle_active(submitted.work.ready_at.clone(), holder, fence, at.into())
}

const fn invalid_execution() -> crate::sandbox::SandboxModelError {
    crate::sandbox::SandboxModelError::invalid_value("sandbox.lifecycle.execution")
}

pub(super) fn custom_authority_preserved(current: &Sandbox, next: &Sandbox) -> bool {
    match (
        &current.lifecycle.custom_runtime,
        &next.lifecycle.custom_runtime,
    ) {
        (None, None) => true,
        (Some(current), Some(next)) => {
            current.migration == next.migration
                && current.source_generation == next.source_generation
                && current.admission_basis == next.admission_basis
        }
        _ => false,
    }
}
