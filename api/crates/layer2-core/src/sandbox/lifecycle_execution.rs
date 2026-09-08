//! Model-owned transitions for one claimed sandbox lifecycle phase.

use super::lifecycle_execution_support::{
    invalid_execution, lifecycle_failure_class, queued_message_phase, valid_runtime_binding,
};
use super::{
    RuntimeBinding, Sandbox, SandboxCheckpointPublication, SandboxCheckpointRef,
    SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxModelError, SandboxPausePhase,
    SandboxRevivePhase, SandboxStatus, SandboxWorkStep, TypedFailure,
};

const MAX_FAILURE_DETAIL_BYTES: usize = 4 * 1024;

impl Sandbox {
    /// Allocate the next checkpoint epoch without mutating checkpoint history.
    pub fn next_checkpoint_epoch(&self) -> Result<u64, SandboxModelError> {
        self.validate()?;
        self.lifecycle
            .checkpoints
            .latest
            .as_ref()
            .map_or(Some(1), |checkpoint| checkpoint.epoch.checked_add(1))
            .ok_or_else(invalid_execution)
    }

    /// Allocate the only epoch a currently claimed lifecycle capture may publish.
    pub fn next_lifecycle_checkpoint_epoch(&self) -> Result<u64, SandboxModelError> {
        self.require_lifecycle_claim()?;
        match self.lifecycle.operation.as_ref() {
            Some(SandboxLifecycleOperation::Pause {
                phase: SandboxPausePhase::CaptureAndPublishCheckpoint,
                checkpoint_epoch: None,
                ..
            })
            | Some(SandboxLifecycleOperation::ImageRotation {
                phase: SandboxImageRotationPhase::CaptureAndPublishCheckpoint,
                checkpoint_epoch: None,
                ..
            }) => {}
            _ => return Err(invalid_execution()),
        }
        self.next_checkpoint_epoch()
    }

    /// Publish a periodic checkpoint without changing active lifecycle work.
    pub fn publish_opportunistic_checkpoint(
        &mut self,
        checkpoint: SandboxCheckpointRef,
        at: String,
    ) -> Result<SandboxCheckpointPublication, SandboxModelError> {
        if self.status != SandboxStatus::Active
            || self.runtime.is_none()
            || self.lifecycle.operation.is_some()
            || self.work.lease.is_some()
            || checkpoint.epoch != self.next_checkpoint_epoch()?
            || crate::seconds_from_rfc3339(&at).is_none()
        {
            return Err(invalid_execution());
        }
        let expected_revision = self.revision.checked_add(1).ok_or_else(invalid_execution)?;
        let mut next = self.clone();
        let publication = next.lifecycle.checkpoints.publish(checkpoint)?;
        if !matches!(publication, SandboxCheckpointPublication::Published { .. }) {
            return Err(invalid_execution());
        }
        next.revision = expected_revision;
        next.updated_at = at;
        next.validate()?;
        *self = next;
        Ok(publication)
    }

    /// Publish a complete held-writer checkpoint and enter agent parking.
    pub fn publish_lifecycle_checkpoint(
        &mut self,
        checkpoint: SandboxCheckpointRef,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<SandboxCheckpointPublication, SandboxModelError> {
        if checkpoint.epoch != self.next_lifecycle_checkpoint_epoch()? {
            return Err(invalid_execution());
        }
        self.apply_claimed(holder, fence, at.clone(), at, |next| {
            let publication = next.lifecycle.checkpoints.publish(checkpoint.clone())?;
            if !matches!(publication, SandboxCheckpointPublication::Published { .. }) {
                return Err(invalid_execution());
            }
            match next.lifecycle.operation.as_mut() {
                Some(SandboxLifecycleOperation::Pause {
                    phase,
                    checkpoint_epoch,
                    ..
                }) if *phase == SandboxPausePhase::CaptureAndPublishCheckpoint
                    && checkpoint_epoch.is_none() =>
                {
                    *phase = SandboxPausePhase::PauseAgents;
                    *checkpoint_epoch = Some(checkpoint.epoch);
                }
                Some(SandboxLifecycleOperation::ImageRotation {
                    phase,
                    checkpoint_epoch,
                    ..
                }) if *phase == SandboxImageRotationPhase::CaptureAndPublishCheckpoint
                    && checkpoint_epoch.is_none() =>
                {
                    *phase = SandboxImageRotationPhase::PauseAgents;
                    *checkpoint_epoch = Some(checkpoint.epoch);
                }
                _ => return Err(invalid_execution()),
            }
            Ok(publication)
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
        if self.lifecycle.checkpoints.latest.as_ref() != Some(verified) {
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

    /// Bind the generation-specific replacement after restore bootstrap succeeds.
    pub fn bind_lifecycle_runtime(
        &mut self,
        binding: RuntimeBinding,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if self.runtime.is_some()
            || !valid_runtime_binding(&binding, &at)
            || self
                .lifecycle
                .runtime_provision
                .as_ref()
                .is_none_or(|reservation| {
                    binding.provision_generation != Some(reservation.generation)
                })
        {
            return Err(invalid_execution());
        }
        self.apply_claimed(holder, fence, at.clone(), at, |next| {
            match next.lifecycle.operation.as_mut() {
                Some(SandboxLifecycleOperation::Revive { phase, .. })
                    if *phase == SandboxRevivePhase::RuntimeProvisioning =>
                {
                    *phase = SandboxRevivePhase::CredentialDelivery;
                }
                Some(SandboxLifecycleOperation::ImageRotation {
                    phase,
                    target_runtime_version,
                    ..
                }) if *phase == SandboxImageRotationPhase::RuntimeProvisioning
                    && *target_runtime_version == binding.runtime_version =>
                {
                    *phase = SandboxImageRotationPhase::CredentialDelivery;
                }
                _ => return Err(invalid_execution()),
            }
            next.runtime = Some(binding);
            next.lifecycle.runtime_provision = None;
            Ok(())
        })
    }

    pub fn complete_lifecycle_credential_delivery(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
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
        self.apply_claimed_with_step(
            holder,
            fence,
            SandboxWorkStep::WaitForCredentialRotation,
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
        self.apply_claimed_with_step(
            holder,
            fence,
            SandboxWorkStep::WaitForCredentialRotation,
            credential_rotation_at,
            at.clone(),
            |next| {
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
                    let checkpoint = next
                        .lifecycle
                        .checkpoints
                        .latest
                        .clone()
                        .ok_or_else(invalid_execution)?;
                    next.lifecycle.rotation.account(&checkpoint)?;
                }
                next.lifecycle.operation = None;
                next.lifecycle.paused_notice = None;
                next.transition(SandboxStatus::Active, at, None)?;
                Ok(())
            },
        )
    }

    /// Fail the current lifecycle with a stable phase-derived class.
    pub fn fail_lifecycle(
        &mut self,
        detail: String,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<TypedFailure, SandboxModelError> {
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
        self.apply_claimed_with_step(
            holder,
            fence,
            SandboxWorkStep::None,
            at.clone(),
            at.clone(),
            |next| {
                next.lifecycle.cancel_operation();
                next.transition(SandboxStatus::Failed, at, Some(failure.clone()))?;
                Ok(())
            },
        )?;
        Ok(failure)
    }
}
