//! Observation deadlines never cancel execution or grant runtime readiness.
use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{Sandbox, SandboxStatus};

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
        Ok(SandboxLifecycleResult::observe(
            sandbox,
            retirement.as_deref(),
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
    pub operation_id: String,
    pub started_at: String,
    pub result_due_at: String,
    pub continuing: bool,
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
        let (operation_id, started_at) = if sandbox.status == SandboxStatus::Retiring {
            let at = retirement_admitted_at?;
            (format!("{}:retire:{at}", sandbox.id), at)
        } else if let Some(operation) = &sandbox.lifecycle.operation {
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
        let due = crate::seconds_from_rfc3339(started_at)?.checked_add(LIFECYCLE_RESULT_SECONDS)?;
        let failed = sandbox.status == SandboxStatus::Failed;
        let overdue = now.duration_since(UNIX_EPOCH).ok()?.as_secs() >= due;
        let state = if failed {
            SandboxLifecycleResultState::Failed
        } else if overdue {
            SandboxLifecycleResultState::RecoverablePending
        } else {
            SandboxLifecycleResultState::InProgress
        };
        let mut actions = vec![SandboxLifecycleAction::Observe];
        if owner_can_retire
            && retirement_admissible(sandbox)
            && !matches!(
                sandbox.status,
                SandboxStatus::Retiring | SandboxStatus::Retired
            )
        {
            actions.push(SandboxLifecycleAction::Retire);
        }
        Some(Self {
            state,
            code: if failed {
                Some("lifecycle_failed")
            } else if overdue {
                Some("lifecycle_wait_exceeded")
            } else {
                None
            },
            operation_id,
            started_at: started_at.into(),
            result_due_at: crate::rfc3339_at(UNIX_EPOCH + Duration::from_secs(due)),
            continuing: !failed,
            actions,
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

#[cfg(test)]
mod tests;
