use serde::{Deserialize, Serialize};

use crate::ids::{SandboxId, SubjectId};
use crate::profiles::SandboxProfileSnapshot;

use super::{
    SandboxCredentialState, SandboxLifecycleState, SandboxModelError, SandboxOrigin,
    SandboxReviveCause, SandboxRotationIdentity, SandboxStatus, SandboxWorkState, SandboxWorkStep,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBinding {
    /// Process identity changes even when the provider reuses its runtime handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    pub runtime_id: String,
    pub runtime_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provision_generation: Option<u64>,
    pub bound_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxLimits {
    pub max_workspaces: u8,
    pub max_agents_per_workspace: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedFailure {
    pub class: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxLifecycleBlocker {
    NotActive,
    NotPaused,
    CheckpointInProgress,
    WorkInProgress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sandbox {
    pub id: SandboxId,
    pub owner: SubjectId,
    pub name: String,
    pub origin: SandboxOrigin,
    pub profile: SandboxProfileSnapshot,
    pub status: SandboxStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<TypedFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeBinding>,
    pub credential: SandboxCredentialState,
    pub work: SandboxWorkState,
    pub limits: SandboxLimits,
    #[serde(default)]
    pub lifecycle: SandboxLifecycleState,
    pub last_activity_at: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purge_after: Option<String>,
    pub revision: u64,
}

impl Sandbox {
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        if let Some(execution) = self.work.execution {
            execution.validate()?;
        }
        if self.name.trim().is_empty() || self.name.len() > 64 {
            return Err(SandboxModelError::invalid_value("sandbox.name"));
        }
        if !(1..=16).contains(&self.limits.max_workspaces)
            || !(1..=16).contains(&self.limits.max_agents_per_workspace)
        {
            return Err(SandboxModelError::invalid_value("sandbox.limits"));
        }
        if self.revision == 0 {
            return Err(SandboxModelError::invalid_value("sandbox.revision"));
        }
        if (self.status == SandboxStatus::Failed) != self.failure.is_some() {
            return Err(SandboxModelError::invalid_value("sandbox.failure"));
        }
        if self
            .work
            .retirement_credential_recovery
            .is_some_and(|recovery| {
                !self.allows_retirement_credential_recovery()
                    || recovery
                        .admitted
                        .is_some_and(|version| !recovery.admits(version))
            })
        {
            return Err(SandboxModelError::invalid_value(
                "sandbox.work.retirement_credential_recovery",
            ));
        }
        if !self.credential.validate()
            || self.work.next_fence == 0
            || self
                .work
                .prepared_installation_digest
                .as_ref()
                .is_some_and(|digest| !super::store::valid_admission_fingerprint(digest))
        {
            return Err(SandboxModelError::invalid_value(
                "sandbox.credential_or_work",
            ));
        }
        self.validate_service_access()?;
        self.lifecycle.validate(self)?;
        self.validate_runtime_generation()?;
        self.validate_checkpoint_capture()?;
        if self.status == SandboxStatus::Paused && self.runtime.is_some() {
            return Err(SandboxModelError::invalid_value("sandbox.runtime"));
        }
        if (self.status == SandboxStatus::Retired) != self.retired_at.is_some() {
            return Err(SandboxModelError::invalid_value("sandbox.retired_at"));
        }
        if self.purge_after.is_some() && self.retired_at.is_none() {
            return Err(SandboxModelError::invalid_value("sandbox.purge_after"));
        }
        Ok(())
    }

    pub fn transition(
        &mut self,
        next: SandboxStatus,
        at: String,
        failure: Option<TypedFailure>,
    ) -> Result<(), SandboxModelError> {
        if (next == SandboxStatus::Failed) != failure.is_some() {
            return Err(SandboxModelError::invalid_value("sandbox.failure"));
        }
        let status = self.status.transition(next)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("sandbox.revision"))?;
        self.status = status;
        self.failure = failure;
        self.updated_at = at.clone();
        if next == SandboxStatus::Retired {
            self.retired_at = Some(at);
        }
        self.revision = revision;
        Ok(())
    }

    pub fn begin_retirement(&mut self, at: String) -> Result<(), SandboxModelError> {
        if self.admitted_retained_runtime_transition().is_some() {
            return Err(SandboxModelError::invalid_value(
                "sandbox.retained_runtime_transition",
            ));
        }
        let mut next = self.clone();
        next.preserve_lifecycle_checkpoint_debt(at.clone())?;
        next.lifecycle.cancel_operation();
        next.transition(SandboxStatus::Retiring, at.clone(), None)?;
        next.work.schedule(SandboxWorkStep::StopAgents, at);
        *self = next;
        Ok(())
    }

    /// Automatic recovery uses the same ordinary owner credential source that admitted the Sandbox.
    pub fn allows_retirement_credential_recovery(&self) -> bool {
        self.status == SandboxStatus::Retiring
            && self.work.step == SandboxWorkStep::RevokeRetirementCredentials
            && self.has_ordinary_credential_scope()
    }

    pub fn has_ordinary_credential_scope(&self) -> bool {
        self.credential.source.is_default_managed()
            && self.credential.service_scope.is_none()
            && self.credential.service_grants.is_empty()
    }

    /// Exhausted retirements keep their deletion intent and outstanding custody.
    /// The terminal work event records the exact step and its last failure.
    pub fn retirement_retry_exhausted(&self) -> bool {
        self.status == SandboxStatus::Retiring
            && self.work.step.is_retirement()
            && self.work.lease.is_none()
            && self.work.ready_at == super::provision::RETIREMENT_STOPPED
    }

    pub(crate) fn retirement_can_resume(&self) -> bool {
        self.retirement_retry_exhausted()
            || (self.work.lease.is_none()
                && self
                    .work
                    .retirement_credential_recovery
                    .is_some_and(|recovery| recovery.admitted.is_none()))
    }

    pub(crate) fn resume_retirement(&mut self, at: String) -> Result<(), SandboxModelError> {
        if !self.retirement_can_resume() || self.retirement_archive_pending() {
            return Err(SandboxModelError::invalid_value("sandbox.work.retirement"));
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("sandbox.revision"))?;
        self.work.retirement_credential_recovery = None;
        self.work.schedule(self.work.step, at.clone());
        self.updated_at = at;
        Ok(())
    }

    /// MarkRetired follows fenced teardown. Require positive evidence of
    /// native allocation cleanup or durable home release before freeing quota.
    pub fn retirement_released_owner_slot(&self) -> bool {
        self.status == SandboxStatus::Retiring
            && self.work.step == SandboxWorkStep::MarkRetired
            && self.runtime.is_none()
            && self.lifecycle.suspended_runtime.is_none()
            && self.credential.current.is_none()
            && self.credential.pending.is_none()
            && !self.credential.delivered
            && self.credential.revoke_debt.is_empty()
            && self.lifecycle.home.as_ref().is_some_and(|home| {
                home.released
                    || home
                        .native_pool
                        .as_ref()
                        .is_some_and(|native| native.cleaned)
            })
    }

    pub fn begin_pause(
        &mut self,
        at: String,
        notice: Option<String>,
    ) -> Result<(), SandboxModelError> {
        if self.pause_eligibility().is_err() {
            return Err(SandboxModelError::invalid_value("sandbox.pause"));
        }
        let mut next = self.clone();
        next.lifecycle.start_pause(&next.id, at.clone(), notice)?;
        next.transition(SandboxStatus::Pausing, at.clone(), None)?;
        next.work
            .schedule(SandboxWorkStep::RunLifecycleOperation, at);
        *self = next;
        Ok(())
    }

    pub fn begin_revive(
        &mut self,
        cause: SandboxReviveCause,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if self.revive_eligibility().is_err() {
            return Err(SandboxModelError::invalid_value("sandbox.revive"));
        }
        let mut next = self.clone();
        next.lifecycle.start_revive(&next.id, at.clone(), cause)?;
        next.transition(SandboxStatus::Reviving, at.clone(), None)?;
        next.work
            .schedule(SandboxWorkStep::RunLifecycleOperation, at);
        *self = next;
        Ok(())
    }

    /// Teardown is irreversible once admitted. A restarted capture authority
    /// may already have parked agents, so it must finish pausing and revive.
    pub fn can_cancel_pause(&self) -> bool {
        self.status == SandboxStatus::Pausing
            && self.runtime.is_some()
            && matches!(
                self.lifecycle.operation.as_ref(),
                Some(super::SandboxLifecycleOperation::Pause { phase: super::SandboxPausePhase::CaptureAndPublishCheckpoint | super::SandboxPausePhase::PauseAgents, authority, .. })
                    if authority.writer_resume_generation == 0
            )
    }

    /// Fence the old pause worker before releasing its exact writer hold.
    pub fn request_pause_cancellation(&mut self, at: String) -> Result<(), SandboxModelError> {
        self.validate()?;
        if !self.can_cancel_pause() || crate::seconds_from_rfc3339(&at).is_none() {
            return Err(SandboxModelError::invalid_value(
                "sandbox.pause_cancellation",
            ));
        }
        let mut next = self.clone();
        match next.lifecycle.operation.as_mut() {
            Some(super::SandboxLifecycleOperation::Pause { phase, .. })
                if matches!(
                    *phase,
                    super::SandboxPausePhase::CaptureAndPublishCheckpoint
                        | super::SandboxPausePhase::PauseAgents
                ) =>
            {
                *phase = super::SandboxPausePhase::ResumeWriters;
            }
            _ => {
                return Err(SandboxModelError::invalid_value(
                    "sandbox.pause_cancellation",
                ))
            }
        }
        next.work
            .schedule(SandboxWorkStep::RunLifecycleOperation, at.clone());
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("sandbox.revision"))?;
        next.updated_at = at;
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// Eligibility to begin a new pause, independent of command replay.
    pub fn pause_eligibility(&self) -> Result<(), SandboxLifecycleBlocker> {
        if self.status != SandboxStatus::Active {
            Err(SandboxLifecycleBlocker::NotActive)
        } else if self.lifecycle.checkpoint_capture.is_some() {
            Err(SandboxLifecycleBlocker::CheckpointInProgress)
        } else if self.work.lease.is_some()
            || !matches!(
                self.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
        {
            Err(SandboxLifecycleBlocker::WorkInProgress)
        } else {
            Ok(())
        }
    }

    /// Eligibility to begin a new revival, independent of command replay.
    pub fn revive_eligibility(&self) -> Result<(), SandboxLifecycleBlocker> {
        if self.status != SandboxStatus::Paused {
            Err(SandboxLifecycleBlocker::NotPaused)
        } else if self.work.lease.is_some() || self.work.step != SandboxWorkStep::None {
            Err(SandboxLifecycleBlocker::WorkInProgress)
        } else {
            Ok(())
        }
    }

    pub fn begin_image_rotation(
        &mut self,
        target_runtime_version: String,
        identity: SandboxRotationIdentity,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.validate_custom_rotation(&target_runtime_version)?;
        let was_paused = self.status == SandboxStatus::Paused;
        if !matches!(self.status, SandboxStatus::Active | SandboxStatus::Paused)
            || (was_paused && self.lifecycle.suspended_runtime.is_none())
            || self.lifecycle.checkpoint_capture.is_some()
            || self.work.lease.is_some()
            || !matches!(
                self.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
        {
            return Err(SandboxModelError::invalid_value("sandbox.image_rotation"));
        }
        let mut next = self.clone();
        next.lifecycle.start_image_rotation(
            &next.id,
            at.clone(),
            target_runtime_version.clone(),
            identity,
        )?;
        if was_paused {
            if let Some(super::SandboxLifecycleOperation::ImageRotation { phase, .. }) =
                &mut next.lifecycle.operation
            {
                *phase = super::SandboxImageRotationPhase::CheckpointVerification;
            }
            next.lifecycle
                .home_recovery
                .as_mut()
                .ok_or_else(|| SandboxModelError::invalid_value("sandbox.runtime_update.home"))?
                .resume_pause = Some(super::SandboxRecoveryPause {
                notice: self.lifecycle.paused_notice.clone(),
            });
        }
        if let Some(request) = next
            .lifecycle
            .runtime_update
            .as_mut()
            .and_then(|state| state.request.as_mut())
            .filter(|r| r.completed_at.is_none())
        {
            if request.target_version != target_runtime_version {
                return Err(SandboxModelError::invalid_value(
                    "sandbox.runtime_update.target",
                ));
            }
            request.started_at = Some(at.clone());
        }
        next.transition(SandboxStatus::Rotating, at.clone(), None)?;
        next.work
            .schedule(SandboxWorkStep::RunLifecycleOperation, at);
        *self = next;
        Ok(())
    }
}
