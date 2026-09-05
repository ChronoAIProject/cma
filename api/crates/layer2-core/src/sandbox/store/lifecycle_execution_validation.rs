use crate::sandbox::{
    SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxPausePhase, SandboxRevivePhase,
};

pub(super) fn checkpoint_operation_advanced(
    current: Option<&SandboxLifecycleOperation>,
    next: Option<&SandboxLifecycleOperation>,
    epoch: u64,
) -> bool {
    match (current, next) {
        (None, None) => true,
        (
            Some(SandboxLifecycleOperation::Pause {
                authority,
                phase: SandboxPausePhase::CaptureAndPublishCheckpoint,
                checkpoint_epoch: None,
            }),
            Some(SandboxLifecycleOperation::Pause {
                authority: next_authority,
                phase: SandboxPausePhase::PauseAgents,
                checkpoint_epoch: Some(next_epoch),
            }),
        ) => authority == next_authority && epoch == *next_epoch,
        (
            Some(SandboxLifecycleOperation::ImageRotation {
                authority,
                phase: SandboxImageRotationPhase::CaptureAndPublishCheckpoint,
                checkpoint_epoch: None,
                target_runtime_version,
                identity,
            }),
            Some(SandboxLifecycleOperation::ImageRotation {
                authority: next_authority,
                phase: SandboxImageRotationPhase::PauseAgents,
                checkpoint_epoch: Some(next_epoch),
                target_runtime_version: next_version,
                identity: next_identity,
            }),
        ) => {
            authority == next_authority
                && epoch == *next_epoch
                && target_runtime_version == next_version
                && identity == next_identity
        }
        _ => false,
    }
}

pub(super) fn operation_advanced(
    current: Option<&SandboxLifecycleOperation>,
    next: Option<&SandboxLifecycleOperation>,
) -> bool {
    match (current, next) {
        (Some(current), Some(next)) => operation_step_matches(current, next),
        (Some(SandboxLifecycleOperation::Pause { phase, .. }), None) => {
            *phase == SandboxPausePhase::PublishPaused
        }
        (Some(SandboxLifecycleOperation::Revive { phase, .. }), None) => {
            *phase == SandboxRevivePhase::PublishActive
        }
        (Some(SandboxLifecycleOperation::ImageRotation { phase, .. }), None) => {
            *phase == SandboxImageRotationPhase::PublishActive
        }
        _ => false,
    }
}

fn operation_step_matches(
    current: &SandboxLifecycleOperation,
    next: &SandboxLifecycleOperation,
) -> bool {
    match (current, next) {
        (
            SandboxLifecycleOperation::Pause {
                authority,
                phase,
                checkpoint_epoch,
            },
            SandboxLifecycleOperation::Pause {
                authority: next_authority,
                phase: next_phase,
                checkpoint_epoch: next_epoch,
            },
        ) => {
            authority == next_authority
                && (*phase == *next_phase || phase.next() == Some(*next_phase))
                && checkpoint_epoch == next_epoch
        }
        (
            SandboxLifecycleOperation::Revive {
                authority,
                phase,
                cause,
                checkpoint_epoch,
            },
            SandboxLifecycleOperation::Revive {
                authority: next_authority,
                phase: next_phase,
                cause: next_cause,
                checkpoint_epoch: next_epoch,
            },
        ) => {
            authority == next_authority
                && (*phase == *next_phase || phase.next() == Some(*next_phase))
                && cause == next_cause
                && checkpoint_epoch == next_epoch
        }
        (
            SandboxLifecycleOperation::ImageRotation {
                authority,
                phase,
                checkpoint_epoch,
                target_runtime_version,
                identity,
            },
            SandboxLifecycleOperation::ImageRotation {
                authority: next_authority,
                phase: next_phase,
                checkpoint_epoch: next_epoch,
                target_runtime_version: next_version,
                identity: next_identity,
            },
        ) => {
            authority == next_authority
                && (*phase == *next_phase || phase.next() == Some(*next_phase))
                && checkpoint_epoch == next_epoch
                && target_runtime_version == next_version
                && identity == next_identity
        }
        _ => false,
    }
}
