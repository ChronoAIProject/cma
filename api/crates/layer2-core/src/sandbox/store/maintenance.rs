//! Atomic, protocol-neutral admission of automatic Sandbox maintenance.

use async_trait::async_trait;

use crate::ids::SandboxId;
use crate::sandbox::{
    image_rotation_required, inactivity_pause_notice, is_inactivity_pause_candidate, Agent,
    Sandbox, SandboxAgentActivity, SandboxEventKind, SandboxLifecycleOperation,
    SandboxRotationIdentity, SandboxStatus,
};
use crate::{seconds_from_rfc3339, MAX_PAUSE_AFTER_SECONDS, MIN_PAUSE_AFTER_SECONDS};

use super::{Page, PageRequest, SandboxStoreError};

const MAX_RUNTIME_VERSION_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxInactivityPauseRequest {
    pub sandbox_id: SandboxId,
    pub requested_at: String,
    pub pause_after_seconds: u64,
    /// When true, historical profile `always_keep_alive` is migrated to the
    /// new per-Sandbox preference: only an explicit `always_on: true` remains.
    pub auto_pause_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxImageRotationRequest {
    pub sandbox_id: SandboxId,
    pub requested_at: String,
    pub expected_runtime_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMaintenanceOutcome {
    Scheduled,
    AlreadyScheduled,
    NotEligible,
}

/// Short lived owner-facing terminal/desktop session leases. The lease is
/// additive evidence for inactivity and is fenced by the store adapter so a
/// concurrent open stream cannot race a pause admission.
#[async_trait]
pub trait SandboxSessionStore: Send + Sync {
    async fn open_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        session_id: &str,
        lease_expires_at: &str,
    ) -> Result<(), SandboxStoreError>;

    async fn renew_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        session_id: &str,
        lease_expires_at: &str,
    ) -> Result<(), SandboxStoreError>;

    async fn close_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        session_id: &str,
    ) -> Result<(), SandboxStoreError>;

    async fn has_open_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        observed_at: &str,
    ) -> Result<bool, SandboxStoreError>;
}

/// A fully decided mutation which an adapter can persist transactionally.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxMaintenanceApplication {
    Persist {
        expected_revision: u64,
        sandbox: Box<Sandbox>,
        event: SandboxEventKind,
    },
    Complete(SandboxMaintenanceOutcome),
}

/// Apply one inactivity decision to a Sandbox and a complete live-Agent projection.
#[doc(hidden)]
pub fn apply_inactivity_pause(
    current: &Sandbox,
    live_agents: &[SandboxAgentActivity],
    request: &SandboxInactivityPauseRequest,
) -> Result<SandboxMaintenanceApplication, SandboxStoreError> {
    validate_inactivity_pause_request(request)?;
    validate_current(current, &request.sandbox_id)?;

    let notice = inactivity_pause_notice(request.pause_after_seconds);
    if matching_pause(current, &notice) {
        return Ok(SandboxMaintenanceApplication::Complete(
            SandboxMaintenanceOutcome::AlreadyScheduled,
        ));
    }
    if !is_inactivity_pause_candidate(
        current,
        live_agents,
        &request.requested_at,
        request.pause_after_seconds,
    ) {
        return Ok(SandboxMaintenanceApplication::Complete(
            SandboxMaintenanceOutcome::NotEligible,
        ));
    }

    schedule(current, SandboxStatus::Pausing, |sandbox| {
        sandbox.begin_pause(request.requested_at.clone(), Some(notice))
    })
}

/// Apply one explicitly requested image update decision to a Sandbox and its complete Agent set.
#[doc(hidden)]
pub fn apply_image_rotation(
    current: &Sandbox,
    agents: &[Agent],
    request: &SandboxImageRotationRequest,
) -> Result<SandboxMaintenanceApplication, SandboxStoreError> {
    validate_image_rotation_request(request)?;
    validate_current(current, &request.sandbox_id)?;

    if matching_rotation(current, &request.expected_runtime_version) {
        return Ok(SandboxMaintenanceApplication::Complete(
            SandboxMaintenanceOutcome::AlreadyScheduled,
        ));
    }
    if !image_rotation_required(current, &request.expected_runtime_version)
        || !crate::sandbox::runtime_update_agents_are_idle(agents)
        || current.runtime_update_request().is_some_and(|update| {
            update.completed_at.is_none()
                && (update.target_version != request.expected_runtime_version
                    || update.failure.is_some())
        })
    {
        return Ok(SandboxMaintenanceApplication::Complete(
            SandboxMaintenanceOutcome::NotEligible,
        ));
    }
    let Ok(identity) = SandboxRotationIdentity::capture(current, agents) else {
        return Ok(SandboxMaintenanceApplication::Complete(
            SandboxMaintenanceOutcome::NotEligible,
        ));
    };

    schedule(current, SandboxStatus::Rotating, |sandbox| {
        sandbox.begin_image_rotation(
            request.expected_runtime_version.clone(),
            identity,
            request.requested_at.clone(),
        )
    })
}

#[doc(hidden)]
pub fn validate_inactivity_pause_request(
    request: &SandboxInactivityPauseRequest,
) -> Result<(), SandboxStoreError> {
    if invalid_common_request(&request.sandbox_id, &request.requested_at)
        || !(MIN_PAUSE_AFTER_SECONDS..=MAX_PAUSE_AFTER_SECONDS)
            .contains(&request.pause_after_seconds)
    {
        return Err(invalid_request());
    }
    Ok(())
}

#[doc(hidden)]
pub fn validate_image_rotation_request(
    request: &SandboxImageRotationRequest,
) -> Result<(), SandboxStoreError> {
    if invalid_common_request(&request.sandbox_id, &request.requested_at)
        || request.expected_runtime_version.trim().is_empty()
        || request.expected_runtime_version.len() > MAX_RUNTIME_VERSION_BYTES
    {
        return Err(invalid_request());
    }
    Ok(())
}

/// Global discovery and evidence-backed atomic maintenance admission.
#[async_trait]
pub trait SandboxMaintenanceStore: Send + Sync {
    /// Page active and paused Sandbox IDs globally in stable, exclusive-cursor ID order.
    async fn sandbox_maintenance_candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<SandboxId>, SandboxStoreError>;

    async fn schedule_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError>;

    /// Evaluate the same evidence and policy fences as an inactivity pause
    /// without committing the resulting lifecycle mutation.
    async fn preview_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError>;

    /// Change the durable automatic-pause exception through an operator-authorized
    /// path. The caller performs capability checks; the store supplies the
    /// same revision compare-and-swap used by owner controls.
    async fn set_sandbox_always_on(
        &self,
        sandbox_id: SandboxId,
        expected_revision: u64,
        always_on: bool,
    ) -> Result<Sandbox, SandboxStoreError>;

    async fn schedule_image_rotation(
        &self,
        request: SandboxImageRotationRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError>;
}

fn schedule(
    current: &Sandbox,
    next_status: SandboxStatus,
    transition: impl FnOnce(&mut Sandbox) -> Result<(), crate::sandbox::SandboxModelError>,
) -> Result<SandboxMaintenanceApplication, SandboxStoreError> {
    let expected_revision = current.revision;
    let mut sandbox = current.clone();
    if transition(&mut sandbox).is_err() {
        return Ok(SandboxMaintenanceApplication::Complete(
            SandboxMaintenanceOutcome::NotEligible,
        ));
    }
    sandbox.validate().map_err(|error| {
        SandboxStoreError::Corrupt(format!("scheduled sandbox is invalid: {error}"))
    })?;
    Ok(SandboxMaintenanceApplication::Persist {
        expected_revision,
        sandbox: Box::new(sandbox),
        event: SandboxEventKind::StatusChanged {
            from: current.status,
            to: next_status,
        },
    })
}

fn validate_current(current: &Sandbox, requested_id: &SandboxId) -> Result<(), SandboxStoreError> {
    if &current.id != requested_id {
        return Err(SandboxStoreError::Corrupt(
            "stored Sandbox key disagrees with its identity".into(),
        ));
    }
    current
        .validate()
        .map_err(|error| SandboxStoreError::Corrupt(format!("stored sandbox is invalid: {error}")))
}

fn matching_pause(sandbox: &Sandbox, notice: &str) -> bool {
    sandbox.status == SandboxStatus::Pausing
        && sandbox.lifecycle.paused_notice.as_deref() == Some(notice)
        && matches!(
            sandbox.lifecycle.operation,
            Some(SandboxLifecycleOperation::Pause { .. })
        )
}

fn matching_rotation(sandbox: &Sandbox, target: &str) -> bool {
    sandbox.status == SandboxStatus::Rotating
        && matches!(
            &sandbox.lifecycle.operation,
            Some(SandboxLifecycleOperation::ImageRotation {
                target_runtime_version,
                ..
            }) if target_runtime_version == target
        )
}

fn invalid_common_request(id: &SandboxId, requested_at: &str) -> bool {
    id.as_str().trim().is_empty() || seconds_from_rfc3339(requested_at).is_none()
}

const fn invalid_request() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("sandbox maintenance request is invalid")
}
