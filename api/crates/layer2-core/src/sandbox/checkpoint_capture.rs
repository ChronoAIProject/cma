//! Durable writer-hold debt for checkpoints that leave a Sandbox running.

use serde::{Deserialize, Serialize};

use super::{
    RuntimeBinding, Sandbox, SandboxCheckpointPublication, SandboxCheckpointRef,
    SandboxLifecycleAuthority, SandboxModelError, SandboxStatus, SandboxWorkState, SandboxWorkStep,
};

const PERIODIC_CHECKPOINT_COOLDOWN_SECONDS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxCheckpointCapturePhase {
    Capture,
    ResumeWriters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCheckpointCapture {
    pub authority: SandboxLifecycleAuthority,
    pub runtime: RuntimeBinding,
    pub checkpoint_epoch: u64,
    pub phase: SandboxCheckpointCapturePhase,
    pub work: SandboxWorkState,
}

impl Sandbox {
    /// Only a confirmed writer release starts the next periodic capture's cooldown.
    pub fn checkpoint_capture_cooldown_elapsed(&self, at: &str) -> bool {
        let Some(now) = crate::seconds_from_rfc3339(at) else {
            return false;
        };
        self.lifecycle
            .checkpoint_capture_completed_at
            .as_deref()
            .is_none_or(|completed| {
                crate::seconds_from_rfc3339(completed)
                    .and_then(|completed| {
                        completed.checked_add(PERIODIC_CHECKPOINT_COOLDOWN_SECONDS)
                    })
                    .is_some_and(|ready| now >= ready)
            })
    }

    pub fn accepts_agent_and_workspace_admission(&self) -> bool {
        self.status == SandboxStatus::Active && self.lifecycle.checkpoint_capture.is_none()
    }

    /// Generic writes may only preserve debt or transfer an existing teardown hold.
    pub fn validate_checkpoint_capture_successor(
        &self,
        next: &Sandbox,
    ) -> Result<(), SandboxModelError> {
        if self.lifecycle.background_checkpoint != next.lifecycle.background_checkpoint
            || self.lifecycle.checkpoint_epoch_high_water
                != next.lifecycle.checkpoint_epoch_high_water
        {
            return Err(invalid_capture());
        }
        if self.lifecycle.checkpoint_capture == next.lifecycle.checkpoint_capture
            && self.lifecycle.checkpoint_capture_completed_at
                == next.lifecycle.checkpoint_capture_completed_at
        {
            return Ok(());
        }
        let mut expected = self.clone();
        if next.status == SandboxStatus::Retiring
            && expected.begin_retirement(next.updated_at.clone()).is_ok()
            && expected == *next
        {
            return Ok(());
        }
        Err(invalid_capture())
    }

    pub(super) fn preserve_lifecycle_checkpoint_debt(
        &mut self,
        at: String,
    ) -> Result<(), SandboxModelError> {
        use super::{
            SandboxImageRotationPhase as Rotation, SandboxLifecycleOperation as Operation,
            SandboxPausePhase as Pause,
        };
        // A retained-home lifecycle never acquired a checkpoint writer hold.
        // Keep existing historical debt, but do not invent debt when retirement
        // preempts a native pause whose provider acknowledgement was lost.
        if self.lifecycle.checkpoint_capture.is_some() || self.retained_home_recovery().is_some() {
            return Ok(());
        }
        let operation = match self.lifecycle.operation.as_ref() {
            Some(
                operation @ Operation::Pause {
                    phase:
                        Pause::CaptureAndPublishCheckpoint
                        | Pause::PauseAgents
                        | Pause::TeardownRuntime
                        | Pause::ResumeWriters,
                    ..
                },
            )
            | Some(
                operation @ Operation::ImageRotation {
                    phase:
                        Rotation::CaptureAndPublishCheckpoint
                        | Rotation::PauseAgents
                        | Rotation::TeardownRuntime,
                    ..
                },
            ) => operation,
            _ => return Ok(()),
        };
        let Some(runtime) = self.runtime.clone() else {
            return Ok(());
        };
        let epoch = operation
            .checkpoint_epoch()
            .map(Ok)
            .unwrap_or_else(|| self.next_checkpoint_epoch())?;
        self.lifecycle.checkpoint_capture = Some(SandboxCheckpointCapture {
            authority: operation.authority().clone(),
            runtime,
            checkpoint_epoch: epoch,
            phase: SandboxCheckpointCapturePhase::ResumeWriters,
            work: SandboxWorkState::new(SandboxWorkStep::RunLifecycleOperation, at),
        });
        Ok(())
    }
    pub fn begin_checkpoint_capture(&mut self, at: String) -> Result<(), SandboxModelError> {
        self.validate()?;
        if !self.checkpoint_capture_cooldown_elapsed(&at)
            || self.status != SandboxStatus::Active
            || self.lifecycle.operation.is_some()
            || self.lifecycle.checkpoint_capture.is_some()
            || self.lifecycle.background_checkpoint.is_some()
            || self.work.lease.is_some()
            || !matches!(
                self.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
            || (self.work.step == SandboxWorkStep::WaitForCredentialRotation
                && self.work.ready_at <= at)
        {
            return Err(invalid_capture());
        }
        let runtime = self.runtime.clone().ok_or_else(invalid_capture)?;
        if runtime.provision_generation.is_none() {
            return Err(invalid_capture());
        }
        let checkpoint_epoch = self.next_checkpoint_epoch()?;
        let mut next = self.clone();
        let authority = next
            .lifecycle
            .allocate_authority(&next.id, at.clone(), true)?;
        next.lifecycle.checkpoint_capture = Some(SandboxCheckpointCapture {
            authority,
            runtime,
            checkpoint_epoch,
            phase: SandboxCheckpointCapturePhase::Capture,
            work: SandboxWorkState::new(SandboxWorkStep::RunLifecycleOperation, at.clone()),
        });
        self.publish_capture_change(next, at)
    }

    pub fn claim_checkpoint_capture(
        &mut self,
        holder: String,
        at: String,
        lease_expires_at: String,
    ) -> Result<(), SandboxModelError> {
        self.validate()?;
        let mut next = self.clone();
        next.lifecycle
            .checkpoint_capture
            .as_mut()
            .ok_or_else(invalid_capture)?
            .work
            .claim(holder, &at, lease_expires_at)?;
        self.publish_capture_change(next, at)
    }

    pub fn renew_checkpoint_capture(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
        lease_expires_at: String,
    ) -> Result<(), SandboxModelError> {
        let capture = self.require_checkpoint_capture_claim(holder, fence)?;
        let lease = capture.work.lease.as_ref().ok_or_else(invalid_capture)?;
        if self.status != SandboxStatus::Active
            || capture.phase != SandboxCheckpointCapturePhase::Capture
            || at < self.updated_at
            || at >= lease.expires_at
            || lease_expires_at <= lease.expires_at
            || crate::seconds_from_rfc3339(&lease_expires_at).is_none()
        {
            return Err(invalid_capture());
        }
        let mut next = self.clone();
        next.lifecycle
            .checkpoint_capture
            .as_mut()
            .ok_or_else(invalid_capture)?
            .work
            .lease
            .as_mut()
            .ok_or_else(invalid_capture)?
            .expires_at = lease_expires_at;
        self.publish_capture_change(next, at)
    }

    /// Publication keeps the exact writer token until resumption is confirmed.
    pub fn publish_captured_checkpoint(
        &mut self,
        checkpoint: SandboxCheckpointRef,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<SandboxCheckpointPublication, SandboxModelError> {
        let capture = self.require_checkpoint_capture_claim(holder, fence)?;
        if self.status != SandboxStatus::Active
            || capture.phase != SandboxCheckpointCapturePhase::Capture
            || checkpoint.epoch != capture.checkpoint_epoch
        {
            return Err(invalid_capture());
        }
        let mut next = self.clone();
        let publication = next.lifecycle.checkpoints.publish(checkpoint)?;
        if !matches!(publication, SandboxCheckpointPublication::Published { .. }) {
            return Err(invalid_capture());
        }
        next.resume_checkpoint_capture(holder, fence, at.clone())?;
        self.publish_capture_change(next, at)?;
        Ok(publication)
    }

    /// Persist the abort/release phase before sending the writer-resume request.
    pub fn require_checkpoint_writer_resume(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.require_checkpoint_capture_claim(holder, fence)?;
        let mut next = self.clone();
        next.resume_checkpoint_capture(holder, fence, at.clone())?;
        self.publish_capture_change(next, at)
    }

    pub fn complete_checkpoint_writer_resume(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        let capture = self.require_checkpoint_capture_claim(holder, fence)?;
        let completed = crate::seconds_from_rfc3339(&at).ok_or_else(invalid_capture)?;
        if capture.phase != SandboxCheckpointCapturePhase::ResumeWriters
            || crate::seconds_from_rfc3339(&self.updated_at).is_none_or(|latest| completed < latest)
            || crate::seconds_from_rfc3339(&capture.authority.requested_at)
                .is_none_or(|requested| completed < requested)
            || self
                .lifecycle
                .checkpoint_capture_completed_at
                .as_deref()
                .and_then(crate::seconds_from_rfc3339)
                .is_some_and(|previous| completed < previous)
        {
            return Err(invalid_capture());
        }
        let mut next = self.clone();
        next.lifecycle.checkpoint_capture = None;
        next.lifecycle.checkpoint_capture_completed_at = Some(at.clone());
        self.publish_capture_change(next, at)
    }

    pub fn retry_checkpoint_capture(
        &mut self,
        ready_at: String,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.require_checkpoint_capture_claim(holder, fence)?;
        if crate::seconds_from_rfc3339(&ready_at).is_none() {
            return Err(invalid_capture());
        }
        let mut next = self.clone();
        next.lifecycle
            .checkpoint_capture
            .as_mut()
            .ok_or_else(invalid_capture)?
            .work
            .retry(holder, fence, ready_at)?;
        self.publish_capture_change(next, at)
    }

    pub(super) fn validate_checkpoint_capture(&self) -> Result<(), SandboxModelError> {
        let Some(capture) = self.lifecycle.checkpoint_capture.as_ref() else {
            return Ok(());
        };
        let authority = &capture.authority;
        let operation_id = format!(
            "sandbox-lifecycle-v1:{}:{}",
            self.id.as_str(),
            authority.generation
        );
        let token = authority.expected_writer_resume_token();
        if authority.operation_id != operation_id
            || authority.generation == 0
            || authority.generation >= self.lifecycle.next_authority
            || authority.writer_resume_token.as_deref() != Some(token.as_str())
            || crate::seconds_from_rfc3339(&authority.requested_at).is_none()
            || capture.checkpoint_epoch == 0
            || self.runtime.as_ref() != Some(&capture.runtime)
            || self.lifecycle.operation.is_some()
            || !matches!(
                self.status,
                SandboxStatus::Active | SandboxStatus::Retiring | SandboxStatus::Failed
            )
            || capture.work.step != SandboxWorkStep::RunLifecycleOperation
            || capture.work.next_fence == 0
            || crate::seconds_from_rfc3339(&capture.work.ready_at).is_none()
            || capture.work.lease.as_ref().is_some_and(|lease| {
                lease.holder.trim().is_empty()
                    || lease.fence == 0
                    || lease.fence >= capture.work.next_fence
                    || crate::seconds_from_rfc3339(&lease.expires_at).is_none()
            })
        {
            return Err(invalid_capture());
        }
        Ok(())
    }

    fn require_checkpoint_capture_claim(
        &self,
        holder: &str,
        fence: u64,
    ) -> Result<&SandboxCheckpointCapture, SandboxModelError> {
        self.validate()?;
        self.lifecycle
            .checkpoint_capture
            .as_ref()
            .filter(|capture| capture.work.owns(holder, fence))
            .ok_or_else(invalid_capture)
    }

    fn resume_checkpoint_capture(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        let capture = self
            .lifecycle
            .checkpoint_capture
            .as_mut()
            .ok_or_else(invalid_capture)?;
        capture.phase = SandboxCheckpointCapturePhase::ResumeWriters;
        capture
            .work
            .advance(holder, fence, SandboxWorkStep::RunLifecycleOperation, at)
    }

    fn publish_capture_change(
        &mut self,
        mut next: Sandbox,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if crate::seconds_from_rfc3339(&at).is_none() {
            return Err(invalid_capture());
        }
        next.revision = self.revision.checked_add(1).ok_or_else(invalid_capture)?;
        next.updated_at = at;
        next.validate()?;
        *self = next;
        Ok(())
    }
}

const fn invalid_capture() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.checkpoint_capture")
}

#[cfg(test)]
#[path = "checkpoint_capture_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "checkpoint_capture_cooldown_tests.rs"]
mod cooldown_tests;
