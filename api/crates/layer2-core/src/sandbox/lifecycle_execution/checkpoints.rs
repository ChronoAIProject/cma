use super::*;

impl Sandbox {
    /// Allocate the next checkpoint epoch without mutating checkpoint history.
    pub fn next_checkpoint_epoch(&self) -> Result<u64, SandboxModelError> {
        self.validate()?;
        self.lifecycle
            .checkpoints
            .latest
            .as_ref()
            .map_or(0, |checkpoint| checkpoint.epoch)
            .max(self.lifecycle.checkpoint_epoch_high_water)
            .checked_add(1)
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
            || self.lifecycle.background_checkpoint.is_some()
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
}
