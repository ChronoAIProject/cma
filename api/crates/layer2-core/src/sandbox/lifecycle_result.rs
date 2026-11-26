//! Observation deadlines never cancel execution or grant runtime readiness.
use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{Sandbox, SandboxLifecycleOperation, SandboxStatus, SandboxWorkStep};

mod admission;
pub use admission::SandboxLifecycleAdmission;

pub const LIFECYCLE_RESULT_SECONDS: u64 = 90;

impl super::SandboxService {
    pub async fn lifecycle_result(
        &self,
        caller: &crate::Caller,
        sandbox: &Sandbox,
    ) -> Result<Option<SandboxLifecycleResult>, super::SandboxServiceError> {
        if sandbox.owner != caller.subject {
            return Ok(None);
        }
        let retirement = if sandbox.status == SandboxStatus::Retiring {
            self.store
                .retirement_admitted_at(&sandbox.id)
                .await
                .map_err(|_| {
                    super::SandboxServiceError::Unavailable(
                        "lifecycle admission unavailable".into(),
                    )
                })?
        } else {
            None
        };
        let failed_admission = if sandbox.status == SandboxStatus::Failed {
            self.store
                .failed_lifecycle_admission(sandbox)
                .await
                .map_err(|_| {
                    super::SandboxServiceError::Unavailable(
                        "lifecycle admission unavailable".into(),
                    )
                })?
        } else {
            None
        };
        Ok(SandboxLifecycleResult::observe_with_admission(
            sandbox,
            retirement.as_deref(),
            failed_admission.as_ref(),
            SystemTime::now(),
            caller.require(crate::Capability::DeleteOwnSandbox).is_ok(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxLifecycleResultState {
    InProgress,
    RecoverablePending,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxLifecycleAction {
    Observe,
    Retire,
}

/// Additive read projection, never persisted inside a strict aggregate reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SandboxLifecycleResult {
    pub state: SandboxLifecycleResultState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_due_at: Option<String>,
    pub continuing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_slot_released: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_retirement_step: Option<&'static str>,
    pub actions: Vec<SandboxLifecycleAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

impl SandboxLifecycleResult {
    /// `retirement_admitted_at` is the atomic Retiring event, not updated_at,
    /// retry scheduling, a worker lease or the observer's first local sighting.
    pub fn observe(
        sandbox: &Sandbox,
        retirement_admitted_at: Option<&str>,
        now: SystemTime,
        owner_can_retire: bool,
    ) -> Option<Self> {
        Self::observe_with_admission(sandbox, retirement_admitted_at, None, now, owner_can_retire)
    }

    pub fn observe_with_admission(
        sandbox: &Sandbox,
        retirement_admitted_at: Option<&str>,
        failed_admission: Option<&SandboxLifecycleAdmission>,
        now: SystemTime,
        owner_can_retire: bool,
    ) -> Option<Self> {
        if sandbox.status == SandboxStatus::Retiring {
            let exhausted = sandbox.retirement_retry_exhausted();
            let mut result = retirement_admitted_at
                .and_then(|at| {
                    Self::from_admission(format!("{}:retire:{at}", sandbox.id), at, now, exhausted)
                })
                .unwrap_or_else(Self::unknown_failure);
            result.owner_slot_released = Some(sandbox.retirement_released_owner_slot());
            if exhausted {
                result.code = Some("retirement_retry_exhausted");
                result.blocked_retirement_step = match sandbox.work.step {
                    SandboxWorkStep::StopAgents => Some("stop_agents"),
                    SandboxWorkStep::RevokeRetirementCredentials => {
                        Some("revoke_retirement_credentials")
                    }
                    SandboxWorkStep::TeardownRuntime => Some("teardown_runtime"),
                    SandboxWorkStep::MarkRetired => Some("mark_retired"),
                    _ => None,
                };
            } else if retirement_admitted_at.is_none() {
                result.state = SandboxLifecycleResultState::InProgress;
                result.code = None;
                result.continuing = true;
                result.retry_after_seconds = Some(5);
            }
            return Some(result);
        }
        let mut result = if sandbox.status == SandboxStatus::Failed {
            failed_admission
                .and_then(|admission| {
                    Self::from_admission(
                        admission.operation_id.clone(),
                        &admission.requested_at,
                        now,
                        true,
                    )
                })
                .unwrap_or_else(Self::unknown_failure)
        } else {
            let (operation_id, started_at) = if let Some(operation) = &sandbox.lifecycle.operation {
                let authority = operation.authority();
                (
                    authority.operation_id.clone(),
                    authority.requested_at.as_str(),
                )
            } else if matches!(
                sandbox.status,
                SandboxStatus::Provisioning | SandboxStatus::Installing
            ) {
                (
                    format!("{}:create:{}", sandbox.id, sandbox.created_at),
                    sandbox.created_at.as_str(),
                )
            } else {
                return None;
            };
            let parked = runtime_update_parked(sandbox);
            let mut result = Self::from_admission(operation_id, started_at, now, parked)?;
            if parked {
                result.code = Some("runtime_update_parked");
            }
            result
        };
        if owner_can_retire
            && retirement_admissible(sandbox)
            && !matches!(
                sandbox.status,
                SandboxStatus::Retiring | SandboxStatus::Retired
            )
        {
            result.actions.push(SandboxLifecycleAction::Retire);
        }
        Some(result)
    }

    fn unknown_failure() -> Self {
        Self {
            state: SandboxLifecycleResultState::Failed,
            code: Some("lifecycle_failed_admission_unavailable"),
            operation_id: None,
            started_at: None,
            result_due_at: None,
            continuing: false,
            owner_slot_released: None,
            blocked_retirement_step: None,
            actions: vec![SandboxLifecycleAction::Observe],
            retry_after_seconds: None,
        }
    }

    /// A queued control is recoverable by observing its original receipt. It
    /// grants no additional resource authority or runtime readiness.
    pub fn queued_control(
        record: &super::control::SandboxControlRecord,
        now: SystemTime,
    ) -> Option<Self> {
        if !record.queued() || record.validate().is_err() {
            return None;
        }
        Self::from_admission(
            record.envelope.command_id.to_string(),
            &record.admitted_at,
            now,
            false,
        )
    }

    fn from_admission(
        operation_id: String,
        started_at: &str,
        now: SystemTime,
        failed: bool,
    ) -> Option<Self> {
        let due = crate::seconds_from_rfc3339(started_at)?.checked_add(LIFECYCLE_RESULT_SECONDS)?;
        let overdue = now.duration_since(UNIX_EPOCH).ok()?.as_secs() >= due;
        let state = if failed {
            SandboxLifecycleResultState::Failed
        } else if overdue {
            SandboxLifecycleResultState::RecoverablePending
        } else {
            SandboxLifecycleResultState::InProgress
        };
        Some(Self {
            state,
            code: if failed {
                Some("lifecycle_failed")
            } else if overdue {
                Some("lifecycle_wait_exceeded")
            } else {
                None
            },
            operation_id: Some(operation_id),
            started_at: Some(started_at.into()),
            result_due_at: Some(crate::rfc3339_at(UNIX_EPOCH + Duration::from_secs(due))),
            continuing: !failed,
            owner_slot_released: None,
            blocked_retirement_step: None,
            actions: vec![SandboxLifecycleAction::Observe],
            retry_after_seconds: (!failed).then_some(5),
        })
    }
}

// Use the same model and successor checks as owner DELETE and its commit.
// Corrupt/held snapshots must not promise a destructive action.
fn retirement_admissible(sandbox: &Sandbox) -> bool {
    let mut next = sandbox.clone();
    sandbox.validate().is_ok()
        && next.begin_retirement(sandbox.updated_at.clone()).is_ok()
        && next.validate().is_ok()
        && sandbox.validate_retirement_archive_successor(&next).is_ok()
}

fn runtime_update_parked(sandbox: &Sandbox) -> bool {
    let Some(request) = sandbox.runtime_update_request() else {
        return false;
    };
    let Some(started_at) = request.started_at.as_deref() else {
        return false;
    };
    if request.completed_at.is_some()
        || request.failure.is_none()
        || sandbox.work.step != SandboxWorkStep::None
        || sandbox.work.lease.is_some()
        || sandbox.admitted_runtime_update_target() != Some(request.target_version.as_str())
    {
        return false;
    }
    match (&sandbox.status, &sandbox.lifecycle.operation) {
        (
            SandboxStatus::Rotating,
            Some(SandboxLifecycleOperation::ImageRotation { authority, .. }),
        ) => {
            started_at == authority.requested_at
                || (!request.automatic
                    && sandbox
                        .lifecycle
                        .unpublished_recovery
                        .as_ref()
                        .is_some_and(|recovery| {
                            recovery.operation_id == authority.operation_id
                                && recovery.validate_for(sandbox).is_ok()
                        })
                    && sandbox
                        .lifecycle
                        .custom_runtime
                        .as_ref()
                        .is_some_and(|custom| custom.completed_at.is_none())
                    && sandbox
                        .validate_custom_recovery_target(&request.target_version)
                        .is_ok()
                    && matches!(
                        (
                            crate::seconds_from_rfc3339(&request.requested_at),
                            crate::seconds_from_rfc3339(started_at),
                            crate::seconds_from_rfc3339(&authority.requested_at),
                        ),
                        (Some(requested), Some(started), Some(recovery))
                            if requested <= started && started < recovery
                    ))
        }
        (SandboxStatus::Reviving, Some(SandboxLifecycleOperation::Revive { .. })) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
