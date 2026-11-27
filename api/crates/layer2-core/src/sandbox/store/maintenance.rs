//! Atomic, protocol-neutral admission of automatic Sandbox maintenance.

use async_trait::async_trait;

use crate::ids::SandboxId;
use crate::sandbox::inactivity::inactivity_pause_refusal;
pub use crate::sandbox::inactivity::InactivityPausePreview;
use crate::sandbox::{
    image_rotation_required, inactivity_pause_notice, is_platform_stop_pause_candidate, Agent,
    InactivityPauseRefusal, Sandbox, SandboxAgentActivity, SandboxEventKind,
    SandboxLifecycleOperation, SandboxRotationIdentity, SandboxStatus, PLATFORM_SPEND_PAUSE_NOTICE,
};
use crate::{seconds_from_rfc3339, MAX_PAUSE_AFTER_SECONDS, MIN_PAUSE_AFTER_SECONDS};

use super::{Page, PageRequest, SandboxStoreError};

const MAX_RUNTIME_VERSION_BYTES: usize = 512;

mod desktop_sessions;
pub use desktop_sessions::DesktopSessionRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxInactivityPauseRequest {
    pub sandbox_id: SandboxId,
    pub requested_at: String,
    pub pause_after_seconds: u64,
    /// When true, historical profile `always_keep_alive` is migrated to the
    /// new per-Sandbox preference: only an explicit `always_on: true` remains.
    pub auto_pause_enabled: bool,
    /// Exact settings observation for operator audit; automatic admission re-reads it transactionally.
    pub expected_policy_revision: Option<u64>,
    pub cause: InactivityPauseCause,
}

/// Why a quiet Sandbox is paused. A platform spend stop is a platform action,
/// not an inactivity policy: it ignores the idle threshold (zero) and shows its
/// own notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InactivityPauseCause {
    #[default]
    Inactivity,
    /// Explicit operator request bypasses enablement grace only.
    OperatorInactivity,
    PlatformSpendStop,
}

impl InactivityPauseCause {
    /// A platform stop overrides every keep-alive preference without changing
    /// it (decision 26); an inactivity pause always honours them.
    pub const fn honours_keep_alive(self) -> bool {
        !matches!(self, Self::PlatformSpendStop)
    }
}

impl SandboxInactivityPauseRequest {
    /// A transaction must read this revision and write-fence the same settings row.
    pub fn matches_policy(&self, stored: &crate::StoredPlatformSettings) -> bool {
        self.policy_refusal(stored).is_none()
    }

    pub fn policy_refusal(
        &self,
        stored: &crate::StoredPlatformSettings,
    ) -> Option<InactivityPauseRefusal> {
        let policy = &stored.settings.inactivity;
        if self
            .expected_policy_revision
            .is_some_and(|revision| revision != stored.revision)
            || !self.auto_pause_enabled
            || !policy.auto_pause_enabled
            || stored.revision == 0
            || self.pause_after_seconds != policy.pause_after_seconds
        {
            return Some(InactivityPauseRefusal::PolicyChanged);
        }
        let Some(now) = seconds_from_rfc3339(&self.requested_at) else {
            return Some(InactivityPauseRefusal::InvalidActivityTime);
        };
        let Some(started) = policy
            .grace_started_at
            .as_deref()
            .and_then(seconds_from_rfc3339)
        else {
            return Some(InactivityPauseRefusal::PolicyChanged);
        };
        (self.cause != InactivityPauseCause::OperatorInactivity
            && now < started.saturating_add(policy.pause_after_seconds))
        .then_some(InactivityPauseRefusal::PolicyGrace)
    }

    fn notice(&self) -> String {
        match self.cause {
            InactivityPauseCause::Inactivity | InactivityPauseCause::OperatorInactivity => {
                inactivity_pause_notice(self.pause_after_seconds)
            }
            InactivityPauseCause::PlatformSpendStop => PLATFORM_SPEND_PAUSE_NOTICE.to_owned(),
        }
    }

    fn eligible(&self, current: &Sandbox, live_agents: &[SandboxAgentActivity]) -> bool {
        match self.cause {
            InactivityPauseCause::Inactivity | InactivityPauseCause::OperatorInactivity => {
                self.evidence_refusal(current, live_agents).is_none()
            }
            InactivityPauseCause::PlatformSpendStop => {
                is_platform_stop_pause_candidate(current, live_agents, &self.requested_at)
            }
        }
    }

    #[doc(hidden)]
    pub fn evidence_refusal(
        &self,
        current: &Sandbox,
        live_agents: &[SandboxAgentActivity],
    ) -> Option<InactivityPauseRefusal> {
        inactivity_pause_refusal(
            current,
            live_agents,
            &self.requested_at,
            self.pause_after_seconds,
        )
    }
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
    /// Replica-mode claims serialize with maintenance and all other desktop opens.
    /// Unsupported adapters refuse; they must never fall back to a local lock.
    async fn acquire_exclusive_sandbox_session(
        &self,
        _: &DesktopSessionRequest,
    ) -> Result<layer3_adapter::DesktopOwnership, SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "shared desktop sessions unavailable".into(),
        ))
    }

    async fn renew_exclusive_sandbox_session(
        &self,
        _: &DesktopSessionRequest,
        _: &layer3_adapter::DesktopOwnership,
    ) -> Result<layer3_adapter::DesktopOwnership, SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "shared desktop sessions unavailable".into(),
        ))
    }

    /// Invoke only after receiver revocation; uncertainty is recovered by expiry.
    async fn release_exclusive_sandbox_session(
        &self,
        _: &layer3_adapter::DesktopOwnership,
    ) -> Result<(), SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "shared desktop sessions unavailable".into(),
        ))
    }

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
    apply_proved_inactivity_pause(current, live_agents, request, false)
}

/// Empty live-Agent evidence is safe only after the store proves the complete
/// child set, pending commands and owner sessions under the same write fences.
#[doc(hidden)]
pub fn apply_proved_inactivity_pause(
    current: &Sandbox,
    live_agents: &[SandboxAgentActivity],
    request: &SandboxInactivityPauseRequest,
    agentless_idle: bool,
) -> Result<SandboxMaintenanceApplication, SandboxStoreError> {
    validate_inactivity_pause_request(request)?;
    validate_current(current, &request.sandbox_id)?;

    let notice = request.notice();
    if matching_pause(current, &notice) {
        return Ok(SandboxMaintenanceApplication::Complete(
            SandboxMaintenanceOutcome::AlreadyScheduled,
        ));
    }
    let proved_empty = live_agents.is_empty()
        && agentless_idle
        && is_platform_stop_pause_candidate(current, live_agents, &request.requested_at);
    if !request.eligible(current, live_agents) && !proved_empty {
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
        || (current.has_managed_desktop_image()
            && (!current.supports_retained_runtime_update()
                || current
                    .runtime_update_request()
                    .is_none_or(|update| update.completed_at.is_some())))
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

/// Plan admission against the complete evidence read by the store transaction.
#[doc(hidden)]
pub fn apply_retained_runtime_transition(
    current: &Sandbox,
    agents: &[Agent],
    request: &crate::sandbox::RetainedRuntimeTransitionRequest,
    requested_at: &str,
) -> Result<SandboxMaintenanceApplication, SandboxStoreError> {
    validate_current(current, &request.sandbox_id)?;
    schedule(current, SandboxStatus::Rotating, |sandbox| {
        sandbox
            .begin_retained_runtime_transition(request.clone(), agents, requested_at.to_owned())
            .map(|_| ())
    })
}

/// Cancel a retained transition while it still has no provider effects.
#[doc(hidden)]
pub fn apply_retained_runtime_abort(
    current: &Sandbox,
    receipt: &crate::sandbox::RetainedRuntimeTransition,
    requested_at: &str,
) -> Result<SandboxMaintenanceApplication, SandboxStoreError> {
    validate_current(current, &receipt.request.sandbox_id)?;
    if current.admitted_retained_runtime_transition() != Some(receipt) {
        return Ok(SandboxMaintenanceApplication::Complete(
            SandboxMaintenanceOutcome::NotEligible,
        ));
    }
    schedule(current, SandboxStatus::Paused, |sandbox| {
        sandbox.abort_unstarted_retained_runtime_transition(
            &receipt.operation_id,
            requested_at.to_owned(),
        )
    })
}

#[doc(hidden)]
pub fn validate_inactivity_pause_request(
    request: &SandboxInactivityPauseRequest,
) -> Result<(), SandboxStoreError> {
    let threshold = match request.cause {
        InactivityPauseCause::Inactivity | InactivityPauseCause::OperatorInactivity => {
            (MIN_PAUSE_AFTER_SECONDS..=MAX_PAUSE_AFTER_SECONDS)
                .contains(&request.pause_after_seconds)
        }
        InactivityPauseCause::PlatformSpendStop => request.pause_after_seconds == 0,
    };
    if invalid_common_request(&request.sandbox_id, &request.requested_at) || !threshold {
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
    /// Operator authority, current activity and lifecycle start share one CAS.
    async fn admit_custom_runtime_migration(
        &self,
        _admission: super::SandboxCustomRuntimeMigrationAdmission,
    ) -> Result<Sandbox, SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "custom runtime migration unavailable".into(),
        ))
    }

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

    /// Return the refusal from the same bounded preview evaluation.
    async fn preview_inactivity_pause_with_reason(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<InactivityPausePreview, SandboxStoreError> {
        let outcome = self.preview_inactivity_pause(request).await?;
        Ok(InactivityPausePreview {
            outcome,
            refusal: (outcome == SandboxMaintenanceOutcome::NotEligible)
                .then_some(InactivityPauseRefusal::EvidenceUnavailable),
        })
    }

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

#[cfg(test)]
#[path = "maintenance_tests.rs"]
mod tests;
