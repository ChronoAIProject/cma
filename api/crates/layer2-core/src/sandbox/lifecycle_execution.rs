//! Model-owned transitions for one claimed sandbox lifecycle phase.
use super::lifecycle_execution_support::{
    invalid_execution, lifecycle_failure_class, queued_message_phase,
};
use super::{
    Sandbox, SandboxCheckpointPublication, SandboxCheckpointRef, SandboxImageRotationPhase,
    SandboxLifecycleOperation, SandboxModelError, SandboxPausePhase, SandboxRevivePhase,
    SandboxStatus, SandboxWorkStep, TypedFailure,
};

const MAX_FAILURE_DETAIL_BYTES: usize = 4 * 1024;

mod checkpoints;
mod runtime_binding;

impl Sandbox {
    /// Advance a durable-home pause without capturing or publishing a CMA
    /// checkpoint. The retained home authority is validated before agents are
    /// parked and the runtime CRD is torn down.
    pub fn complete_lifecycle_persistent_pause(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.apply_claimed(holder, fence, at.clone(), at, |next| {
            if next.lifecycle.home_recovery.is_none() {
                return Err(invalid_execution());
            }
            match next.lifecycle.operation.as_mut() {
                Some(SandboxLifecycleOperation::Pause { phase, .. })
                    if *phase == SandboxPausePhase::CaptureAndPublishCheckpoint =>
                {
                    *phase = SandboxPausePhase::PauseAgents
                }
                Some(SandboxLifecycleOperation::ImageRotation { phase, .. })
                    if *phase == SandboxImageRotationPhase::CaptureAndPublishCheckpoint =>
                {
                    *phase = SandboxImageRotationPhase::PauseAgents
                }
                _ => return Err(invalid_execution()),
            }
            Ok(())
        })
    }

    /// Record that every live agent has been durably parked.
    pub fn complete_lifecycle_agent_pause(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.apply_claimed(holder, fence, at.clone(), at, |next| {
            match next.lifecycle.operation.as_mut() {
                Some(SandboxLifecycleOperation::Pause { phase, .. })
                    if *phase == SandboxPausePhase::PauseAgents =>
                {
                    *phase = SandboxPausePhase::TeardownRuntime;
                }
                Some(SandboxLifecycleOperation::ImageRotation { phase, .. })
                    if *phase == SandboxImageRotationPhase::PauseAgents =>
                {
                    *phase = SandboxImageRotationPhase::TeardownRuntime;
                }
                _ => return Err(invalid_execution()),
            }
            Ok(())
        })
    }

    /// Remove the old runtime binding only after provider teardown is confirmed.
    pub fn complete_lifecycle_runtime_teardown(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if self.runtime.is_none() {
            return Err(invalid_execution());
        }
        self.apply_claimed(holder, fence, at.clone(), at, |next| {
            match next.lifecycle.operation.as_mut() {
                Some(SandboxLifecycleOperation::Pause { phase, .. })
                    if *phase == SandboxPausePhase::TeardownRuntime =>
                {
                    *phase = SandboxPausePhase::PublishPaused;
                }
                Some(SandboxLifecycleOperation::ImageRotation { phase, .. })
                    if *phase == SandboxImageRotationPhase::TeardownRuntime =>
                {
                    *phase = SandboxImageRotationPhase::CheckpointVerification;
                }
                _ => return Err(invalid_execution()),
            }
            next.runtime = None;
            Ok(())
        })
    }

    /// Advance only after the pinned latest checkpoint has been verified.
    pub fn complete_lifecycle_checkpoint_verification(
        &mut self,
        verified: &SandboxCheckpointRef,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if self.lifecycle.home_recovery.is_some()
            || self.lifecycle.checkpoints.latest.as_ref() != Some(verified)
        {
            return Err(invalid_execution());
        }
        self.advance_recovery_phase(
            SandboxRevivePhase::CheckpointVerification,
            SandboxRevivePhase::RuntimeProvisioning,
            SandboxImageRotationPhase::CheckpointVerification,
            SandboxImageRotationPhase::RuntimeProvisioning,
            holder,
            fence,
            at,
        )
    }

    pub fn complete_lifecycle_credential_delivery(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if self.needs_service_access_credential() {
            return Err(invalid_execution());
        }
        self.advance_recovery_phase(
            SandboxRevivePhase::CredentialDelivery,
            SandboxRevivePhase::HomeRestore,
            SandboxImageRotationPhase::CredentialDelivery,
            SandboxImageRotationPhase::HomeRestore,
            holder,
            fence,
            at,
        )
    }

    pub fn complete_lifecycle_home_restore(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.advance_recovery_phase(
            SandboxRevivePhase::HomeRestore,
            SandboxRevivePhase::ConversationReopen,
            SandboxImageRotationPhase::HomeRestore,
            SandboxImageRotationPhase::ConversationReopen,
            holder,
            fence,
            at,
        )
    }

    pub fn complete_lifecycle_conversation_reopen(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.advance_recovery_phase(
            SandboxRevivePhase::ConversationReopen,
            SandboxRevivePhase::QueuedMessageDelivery,
            SandboxImageRotationPhase::ConversationReopen,
            SandboxImageRotationPhase::QueuedMessageDelivery,
            holder,
            fence,
            at,
        )
    }

    /// Release one queue-item claim without advancing the enclosing phase.
    pub fn publish_lifecycle_queue_progress(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.apply_claimed(holder, fence, at.clone(), at, |next| {
            if !queued_message_phase(next.lifecycle.operation.as_ref()) {
                return Err(invalid_execution());
            }
            Ok(())
        })
    }

    /// Release a transiently blocked phase for a fenced retry.
    pub fn retry_lifecycle(
        &mut self,
        ready_at: String,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.require_lifecycle_authority(holder, fence)?;
        if crate::seconds_from_rfc3339(&ready_at).is_none()
            || crate::seconds_from_rfc3339(&at).is_none()
        {
            return Err(invalid_execution());
        }
        let mut next = self.clone();
        next.work.retry(holder, fence, ready_at)?;
        next.revision = self.revision.checked_add(1).ok_or_else(invalid_execution)?;
        next.updated_at = at;
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn complete_lifecycle_queued_message_delivery(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.advance_recovery_phase(
            SandboxRevivePhase::QueuedMessageDelivery,
            SandboxRevivePhase::PublishActive,
            SandboxImageRotationPhase::QueuedMessageDelivery,
            SandboxImageRotationPhase::PublishActive,
            holder,
            fence,
            at,
        )
    }

    pub fn publish_lifecycle_paused(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.apply_claimed_with_step(
            holder,
            fence,
            SandboxWorkStep::None,
            at.clone(),
            at.clone(),
            |next| {
                if !matches!(
                    next.lifecycle.operation,
                    Some(SandboxLifecycleOperation::Pause {
                        phase: SandboxPausePhase::PublishPaused,
                        ..
                    })
                ) || next.runtime.is_some()
                {
                    return Err(invalid_execution());
                }
                next.lifecycle.operation = None;
                next.transition(SandboxStatus::Paused, at, None)?;
                Ok(())
            },
        )
    }

    /// Publish the unchanged live runtime only after exact-token writer resumption.
    pub fn complete_pause_cancellation(
        &mut self,
        credential_rotation_at: String,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        let next_step = if self.credential.source.is_managed() {
            SandboxWorkStep::WaitForCredentialRotation
        } else {
            SandboxWorkStep::None
        };
        self.apply_claimed_with_step(
            holder,
            fence,
            next_step,
            credential_rotation_at,
            at.clone(),
            |next| {
                if next.runtime.is_none()
                    || !matches!(
                        next.lifecycle.operation,
                        Some(SandboxLifecycleOperation::Pause {
                            phase: SandboxPausePhase::ResumeWriters,
                            ..
                        })
                    )
                {
                    return Err(invalid_execution());
                }
                next.lifecycle.cancel_operation();
                next.lifecycle.paused_notice = None;
                if crate::seconds_from_rfc3339(&at)
                    > crate::seconds_from_rfc3339(&next.last_activity_at)
                {
                    next.last_activity_at = at.clone();
                }
                next.transition(SandboxStatus::Active, at, None)
            },
        )
    }

    pub fn publish_lifecycle_active(
        &mut self,
        credential_rotation_at: String,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if crate::seconds_from_rfc3339(&credential_rotation_at).is_none() {
            return Err(invalid_execution());
        }
        let resume_pause = self
            .lifecycle
            .home_recovery
            .as_ref()
            .and_then(|source| source.resume_pause.clone());
        let next_step = if resume_pause.is_some() {
            SandboxWorkStep::RunLifecycleOperation
        } else if !self.credential.source.is_managed() {
            SandboxWorkStep::None
        } else {
            SandboxWorkStep::WaitForCredentialRotation
        };
        let ready_at = if resume_pause.is_some() {
            at.clone()
        } else {
            credential_rotation_at
        };
        self.apply_claimed_with_step(holder, fence, next_step, ready_at, at.clone(), |next| {
            let rotation = matches!(
                next.lifecycle.operation,
                Some(SandboxLifecycleOperation::ImageRotation {
                    phase: SandboxImageRotationPhase::PublishActive,
                    ..
                })
            );
            let publishable = rotation
                || matches!(
                    next.lifecycle.operation,
                    Some(SandboxLifecycleOperation::Revive {
                        phase: SandboxRevivePhase::PublishActive,
                        ..
                    })
                );
            if !publishable || next.runtime.is_none() {
                return Err(invalid_execution());
            }
            if rotation {
                if next.lifecycle.home_recovery.is_some() {
                    let boundary = next
                        .lifecycle
                        .operation
                        .as_ref()
                        .ok_or_else(invalid_execution)?
                        .authority()
                        .requested_at
                        .clone();
                    next.lifecycle.rotation.account_at(&boundary)?;
                } else {
                    let checkpoint = next
                        .lifecycle
                        .checkpoints
                        .latest
                        .clone()
                        .ok_or_else(invalid_execution)?;
                    next.lifecycle.rotation.account(&checkpoint)?;
                }
            }
            next.complete_runtime_update_publication(&at)?;
            next.lifecycle.operation = None;
            next.lifecycle.home_recovery = None;
            next.lifecycle.unpublished_recovery = None;
            next.lifecycle.unpublished_recovery_attempted = false;
            next.lifecycle.paused_notice = None;
            if let Some(pause) = resume_pause {
                next.lifecycle
                    .start_pause(&next.id, at.clone(), pause.notice)?;
                next.transition(SandboxStatus::Pausing, at, None)?;
            } else {
                next.transition(SandboxStatus::Active, at, None)?;
            }
            Ok(())
        })
    }

    /// Fail with a phase-derived class, preserving uncompleted retained allocation evidence.
    pub fn fail_lifecycle(
        &mut self,
        detail: String,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<TypedFailure, SandboxModelError> {
        if self.lifecycle.unpublished_recovery_attempted
            || self.lifecycle.unpublished_recovery.is_some()
        {
            return Err(invalid_execution());
        }
        if detail.trim().is_empty()
            || detail.len() > MAX_FAILURE_DETAIL_BYTES
            || detail.chars().any(char::is_control)
        {
            return Err(invalid_execution());
        }
        let class = lifecycle_failure_class(
            self.lifecycle
                .operation
                .as_ref()
                .ok_or_else(invalid_execution)?,
        );
        let failure = class.typed_failure(detail);
        let failed_provision = self.failed_provision_evidence();
        self.apply_claimed_with_step(
            holder,
            fence,
            SandboxWorkStep::None,
            at.clone(),
            at.clone(),
            |next| {
                next.lifecycle.cancel_operation();
                next.lifecycle.failed_provision = failed_provision;
                next.transition(SandboxStatus::Failed, at, Some(failure.clone()))?;
                Ok(())
            },
        )?;
        Ok(failure)
    }
}
