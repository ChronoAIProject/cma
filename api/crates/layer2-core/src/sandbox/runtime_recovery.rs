//! Aggregate transitions after exact runtime absence has been established.

use super::{
    Agent, Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxModelError,
    SandboxPausePhase, SandboxReviveCause, SandboxReviveFailureClass, SandboxRotationIdentity,
    SandboxStatus, SandboxWorkStep,
};

impl Sandbox {
    /// Unbound provisioning/recovery steps are intentional and are not runtime loss.
    pub fn requires_runtime_loss_recovery(&self) -> bool {
        // A just-assigned warm runtime has not delivered user credentials or
        // started child creation. Its provisioning worker reserves a replacement
        // generation; checkpoint-based recovery would incorrectly fail it.
        if self.status == SandboxStatus::Provisioning
            && self.work.step == super::SandboxWorkStep::ProvisionRuntime
            && self.credential.key_generation == 0
            && !self.credential.delivered
            && self.credential.current.is_none()
            && self.credential.pending.is_none()
        {
            return false;
        }
        self.status == SandboxStatus::Active
            || (self.runtime.is_some()
                && matches!(
                    self.status,
                    SandboxStatus::Provisioning
                        | SandboxStatus::Installing
                        | SandboxStatus::Pausing
                        | SandboxStatus::Reviving
                        | SandboxStatus::Rotating
                        | SandboxStatus::CredentialRotating
                        | SandboxStatus::Retiring
                ))
    }

    #[cfg(test)]
    pub(crate) fn recover_runtime_loss(
        &mut self,
        recovered_agents: &[Agent],
        retain_runtime: bool,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.recover_runtime_loss_with_policy(recovered_agents, retain_runtime, at, false)
    }

    /// Reset runtime authority while preserving checkpoint history and lifecycle purpose.
    pub(crate) fn recover_runtime_loss_with_policy(
        &mut self,
        recovered_agents: &[Agent],
        retain_runtime: bool,
        at: String,
        rotation_recovery: bool,
    ) -> Result<(), SandboxModelError> {
        self.validate()?;
        if !self.requires_runtime_loss_recovery() || crate::seconds_from_rfc3339(&at).is_none() {
            return Err(invalid_recovery());
        }
        let mut next = self.clone();
        next.lifecycle.checkpoint_capture = None;
        next.runtime = None;
        if self.status == SandboxStatus::Retiring {
            // Runtime absence settles process authority, but snapshot and home
            // deletion remain durable retirement work.
            next.lifecycle.cancel_operation();
            next.work.schedule(SandboxWorkStep::StopAgents, at.clone());
            next.updated_at = at;
            next.revision = self.revision.checked_add(1).ok_or_else(invalid_recovery)?;
            next.validate()?;
            *self = next;
            return Ok(());
        }
        next.work
            .schedule(SandboxWorkStep::RunLifecycleOperation, at.clone());
        if self.lifecycle.home.is_some() && self.credential.current.is_some() {
            if retain_runtime {
                // Preserve storage-owning provider authority after process loss.
                // Its worker must quiesce the old incarnation before replacement.
                next.lifecycle.suspended_runtime =
                    Some(Box::new(self.runtime.clone().ok_or_else(invalid_recovery)?));
            }
            next.recover_retained_home(self, recovered_agents, &at, rotation_recovery)?;
            next.updated_at = at;
            next.revision = self.revision.checked_add(1).ok_or_else(invalid_recovery)?;
            next.validate()?;
            *self = next;
            return Ok(());
        }
        match next.lifecycle.checkpoints.latest.as_ref() {
            None => {
                next.lifecycle.cancel_operation();
                next.transition(
                    SandboxStatus::Failed,
                    at.clone(),
                    Some(SandboxReviveFailureClass::RuntimeLost.typed_failure(
                        "the sandbox runtime was lost before a complete checkpoint was saved",
                    )),
                )?;
                next.work.schedule(SandboxWorkStep::None, at.clone());
            }
            Some(checkpoint) => {
                let epoch = checkpoint.epoch;
                let notice = format!(
                    "Your sandbox runtime was lost. Your sandbox is being restored from the checkpoint saved at {}.",
                    checkpoint.sealed_at
                );
                match &self.lifecycle.operation {
                    Some(SandboxLifecycleOperation::Pause { .. }) => {
                        next.lifecycle.start_pause(
                            &next.id,
                            at.clone(),
                            self.lifecycle.paused_notice.clone(),
                        )?;
                        if let Some(SandboxLifecycleOperation::Pause {
                            phase,
                            checkpoint_epoch,
                            ..
                        }) = next.lifecycle.operation.as_mut()
                        {
                            *phase = SandboxPausePhase::PublishPaused;
                            *checkpoint_epoch = Some(epoch);
                        }
                    }
                    Some(SandboxLifecycleOperation::ImageRotation {
                        target_runtime_version,
                        ..
                    }) => {
                        let identity = SandboxRotationIdentity::capture(&next, recovered_agents)?;
                        next.lifecycle.start_image_rotation(
                            &next.id,
                            at.clone(),
                            target_runtime_version.clone(),
                            identity,
                        )?;
                        if let Some(SandboxLifecycleOperation::ImageRotation {
                            phase,
                            checkpoint_epoch,
                            ..
                        }) = next.lifecycle.operation.as_mut()
                        {
                            *phase = SandboxImageRotationPhase::CheckpointVerification;
                            *checkpoint_epoch = Some(epoch);
                        }
                        if rotation_recovery {
                            next.sync_rotation_request_start(&at, target_runtime_version);
                        }
                        next.lifecycle.paused_notice = Some(notice);
                    }
                    _ => {
                        next.lifecycle.start_revive(
                            &next.id,
                            at.clone(),
                            SandboxReviveCause::RuntimeLost,
                        )?;
                        if next.status != SandboxStatus::Reviving {
                            next.transition(SandboxStatus::Reviving, at.clone(), None)?;
                        }
                        next.lifecycle.paused_notice = Some(notice);
                    }
                }
            }
        }
        next.updated_at = at;
        next.revision = self.revision.checked_add(1).ok_or_else(invalid_recovery)?;
        next.validate()?;
        *self = next;
        Ok(())
    }
}

impl Sandbox {
    fn recover_retained_home(
        &mut self,
        previous: &Sandbox,
        agents: &[Agent],
        at: &str,
        rotation_recovery: bool,
    ) -> Result<(), SandboxModelError> {
        match &previous.lifecycle.operation {
            Some(SandboxLifecycleOperation::ImageRotation {
                target_runtime_version,
                ..
            }) => {
                let identity = SandboxRotationIdentity::capture(self, agents)?;
                let resume_pause = previous
                    .retained_home_recovery()
                    .and_then(|source| source.resume_pause.clone());
                self.lifecycle.start_image_rotation(
                    &self.id,
                    at.into(),
                    target_runtime_version.clone(),
                    identity,
                )?;
                if let Some(SandboxLifecycleOperation::ImageRotation {
                    phase,
                    checkpoint_epoch,
                    ..
                }) = self.lifecycle.operation.as_mut()
                {
                    *phase = SandboxImageRotationPhase::CheckpointVerification;
                    *checkpoint_epoch = previous
                        .lifecycle
                        .checkpoints
                        .latest
                        .as_ref()
                        .map(|checkpoint| checkpoint.epoch);
                }
                if rotation_recovery {
                    self.sync_rotation_request_start(at, target_runtime_version);
                }
                if let Some(source) = &mut self.lifecycle.home_recovery {
                    source.resume_pause = resume_pause;
                }
            }
            _ => {
                let resume_pause = match &previous.lifecycle.operation {
                    Some(SandboxLifecycleOperation::Pause { phase, .. })
                        if *phase != SandboxPausePhase::ResumeWriters =>
                    {
                        Some(super::SandboxRecoveryPause {
                            notice: previous.lifecycle.paused_notice.clone(),
                        })
                    }
                    _ => previous
                        .lifecycle
                        .home_recovery
                        .as_ref()
                        .and_then(|source| source.resume_pause.clone()),
                };
                self.lifecycle.start_revive(
                    &self.id,
                    at.into(),
                    SandboxReviveCause::RuntimeLost,
                )?;
                self.lifecycle
                    .home_recovery
                    .as_mut()
                    .ok_or_else(invalid_recovery)?
                    .resume_pause = resume_pause;
                if self.status != SandboxStatus::Reviving {
                    self.transition(SandboxStatus::Reviving, at.into(), None)?;
                }
            }
        }
        self.lifecycle.paused_notice = Some(
            "Your sandbox runtime was interrupted. Your saved environment and original conversations are being reconnected.".into(),
        );
        Ok(())
    }

    fn sync_rotation_request_start(&mut self, at: &str, target: &str) {
        if let Some(request) = self
            .lifecycle
            .runtime_update
            .as_mut()
            .and_then(|state| state.request.as_mut())
            .filter(|request| request.completed_at.is_none() && request.target_version == target)
        {
            request.started_at = Some(at.into());
        }
    }
}

const fn invalid_recovery() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.runtime_loss_recovery")
}
