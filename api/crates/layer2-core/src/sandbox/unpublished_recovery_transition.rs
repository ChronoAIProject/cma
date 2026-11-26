//! Preserve the exact retained home and all child records during failed-open recovery.
use super::{
    Agent, Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxModelError,
    SandboxRotationIdentity, SandboxRuntimeUpdateRequest, SandboxRuntimeUpdateState, SandboxStatus,
    SandboxUnpublishedRecoveryProof, SandboxUnpublishedRecoveryState, SandboxWorkStep,
};

impl Sandbox {
    pub(super) fn begin_unpublished_runtime_recovery(
        &self,
        agents: &[Agent],
        proof: &SandboxUnpublishedRecoveryProof,
        target: &str,
        automatic: bool,
        at: &str,
    ) -> Result<Self, SandboxModelError> {
        let invalid = || SandboxModelError::invalid_value("sandbox.unpublished_runtime_recovery");
        // Explicit scoped restart needs a new activity admission. Existing
        // automatic settlement keeps its exact target and once-only checks.
        if !automatic && self.lifecycle.runtime_migration_admission.is_some() {
            return Err(invalid());
        }
        self.validate_custom_recovery_target(target)?;
        let mut next = self.clone();
        let previous = next
            .runtime
            .take()
            .or_else(|| {
                self.unpublished_recovery_binding()
                    .filter(|_| {
                        agents.is_empty() && proof.boot_id.is_none() && proof.runtime_process_absent
                    })
                    .cloned()
            })
            .ok_or_else(invalid)?;
        let resume_pause = next
            .retained_home_recovery()
            .or_else(|| {
                next.lifecycle
                    .failed_provision
                    .as_ref()
                    .map(|failed| &failed.home)
            })
            .and_then(|source| source.resume_pause.clone());
        next.lifecycle.suspended_runtime = Some(Box::new(previous));
        let identity = SandboxRotationIdentity::capture(&next, agents)?;
        next.lifecycle
            .start_image_rotation(&next.id, at.into(), target.into(), identity)?;
        let Some(SandboxLifecycleOperation::ImageRotation {
            phase, authority, ..
        }) = &mut next.lifecycle.operation
        else {
            return Err(invalid());
        };
        *phase = SandboxImageRotationPhase::CheckpointVerification;
        next.lifecycle.unpublished_recovery = Some(Box::new(SandboxUnpublishedRecoveryState {
            operation_id: authority.operation_id.clone(),
            proof: Box::new(proof.clone()),
            quiescence: None,
        }));
        next.lifecycle.unpublished_recovery_attempted = true;
        let source = next.lifecycle.home_recovery.as_mut().ok_or_else(invalid)?;
        // A prior uncompleted reservation can be above the exact provider source.
        // Keep that high-water fence while retaining the proved original binding.
        source.source_generation = proof.runtime_generation;
        source.resume_pause = resume_pause;
        // This narrowly admitted transition is independent of ordinary Failed
        // eligibility. It never broadens begin_revive or the general status graph.
        next.status = SandboxStatus::Rotating;
        next.failure = None;
        next.lifecycle.paused_notice = Some(
            "Your saved environment and original conversations are being reopened after runtime startup failed.".into(),
        );
        next.lifecycle.runtime_update = Some(SandboxRuntimeUpdateState {
            checked_at: at.into(),
            observed_version: target.into(),
            request: Some(SandboxRuntimeUpdateRequest {
                target_version: target.into(),
                requested_at: at.into(),
                automatic,
                started_at: Some(at.into()),
                completed_at: None,
                failure: None,
                retained_transition: None,
            }),
        });
        next.preserve_custom_recovery_request(self)?;
        next.work
            .schedule(SandboxWorkStep::RunLifecycleOperation, at.into());
        next.updated_at = at.into();
        next.revision = next.revision.checked_add(1).ok_or_else(invalid)?;
        next.validate()?;
        Ok(next)
    }
}
