//! Durable, owner-visible requests to replace runtime software on the same home.
use serde::{Deserialize, Serialize};

use super::{
    Agent, AgentStatus, RuntimeBinding, Sandbox, SandboxLifecycleOperation, SandboxModelError,
    SandboxStatus, SandboxWorkStep, TypedFailure,
};
use crate::seconds_from_rfc3339;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRuntimeUpdateState {
    pub checked_at: String,
    pub observed_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<SandboxRuntimeUpdateRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRuntimeUpdateRequest {
    pub target_version: String,
    pub requested_at: String,
    pub automatic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<TypedFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_transition: Option<Box<super::RetainedRuntimeTransition>>,
}

impl SandboxRuntimeUpdateState {
    pub(super) fn validate(&self) -> Result<(), SandboxModelError> {
        if seconds_from_rfc3339(&self.checked_at).is_none()
            || !valid_version(&self.observed_version)
        {
            return Err(invalid());
        }
        if let Some(request) = &self.request {
            if !valid_version(&request.target_version)
                || seconds_from_rfc3339(&request.requested_at).is_none()
                || [&request.started_at, &request.completed_at]
                    .into_iter()
                    .flatten()
                    .any(|at| seconds_from_rfc3339(at).is_none())
                || (request.completed_at.is_some()
                    && (request.started_at.is_none() || request.failure.is_some()))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

impl Sandbox {
    pub fn retained_runtime_binding(&self) -> Option<&RuntimeBinding> {
        self.runtime
            .as_ref()
            .or(self.lifecycle.suspended_runtime.as_deref())
    }

    pub fn supports_retained_runtime_update(&self) -> bool {
        self.has_managed_runtime_image()
            && self.lifecycle.home.as_ref().is_some_and(|home| {
                !home.released
                    && home.native_pool.is_some()
                    && home
                        .attachment()
                        .is_some_and(|attachment| attachment.request.policy.is_native_vct())
            })
            && self.retained_runtime_binding().is_some()
    }

    /// Once a scoped startup policy has been applied, changing its image or
    /// policy requires another scoped retained transition on the same home.
    pub fn requires_retained_runtime_transition(&self) -> bool {
        self.lifecycle.retained_startup_policy.is_some()
    }

    pub fn runtime_update_request(&self) -> Option<&SandboxRuntimeUpdateRequest> {
        self.lifecycle
            .runtime_update
            .as_ref()
            .and_then(|state| state.request.as_ref())
    }

    /// Only an admitted lifecycle operation can change the retained image.
    /// Ordinary recovery keeps the prior image unless an owner update was pinned.
    pub fn admitted_runtime_update_target(&self) -> Option<&str> {
        match self.lifecycle.operation.as_ref()? {
            SandboxLifecycleOperation::ImageRotation {
                target_runtime_version,
                ..
            } => Some(target_runtime_version),
            SandboxLifecycleOperation::Revive { .. } => self
                .runtime_update_request()
                .filter(|request| request.started_at.is_some() && request.completed_at.is_none())
                .map(|request| request.target_version.as_str()),
            _ => None,
        }
    }

    /// Queueing does not stop a turn or alter runtime authority.
    pub fn request_runtime_update(
        &mut self,
        target: String,
        automatic: bool,
        at: String,
    ) -> Result<bool, SandboxModelError> {
        self.validate()?;
        if !self.has_managed_runtime_image()
            || self.requires_retained_runtime_transition()
            || !valid_version(&target)
            || seconds_from_rfc3339(&at).is_none()
        {
            return Err(invalid());
        }
        if let Some(request) = self
            .runtime_update_request()
            .filter(|r| r.completed_at.is_none())
        {
            if request.retained_transition.is_some() || request.target_version != target {
                return Err(invalid());
            }
            if request.failure.is_none() {
                return Ok(false);
            }
            // Retained scoped clearance authorizes the original attempt only.
            // An ordinary explicit retry cannot renew that activity admission.
            if self.lifecycle.runtime_migration_admission.is_some()
                || self.work.lease.is_some()
                || self.admitted_runtime_update_target() != Some(target.as_str())
            {
                return Err(invalid());
            }
            self.lifecycle
                .runtime_update
                .as_mut()
                .and_then(|s| s.request.as_mut())
                .ok_or_else(invalid)?
                .failure = None;
            self.work
                .schedule(SandboxWorkStep::RunLifecycleOperation, at.clone());
            self.touch_runtime_update(at)?;
            return Ok(true);
        }
        if self
            .retained_runtime_binding()
            .is_some_and(|binding| binding.runtime_version == target)
        {
            return Ok(false);
        }
        if !self.supports_retained_runtime_update()
            || !matches!(self.status, SandboxStatus::Active | SandboxStatus::Paused)
        {
            return Err(invalid());
        }
        let state =
            self.lifecycle
                .runtime_update
                .get_or_insert_with(|| SandboxRuntimeUpdateState {
                    checked_at: at.clone(),
                    observed_version: target.clone(),
                    request: None,
                });
        state.request = Some(SandboxRuntimeUpdateRequest {
            target_version: target,
            requested_at: at.clone(),
            automatic,
            started_at: None,
            completed_at: None,
            failure: None,
            retained_transition: None,
        });
        self.touch_runtime_update(at)?;
        Ok(true)
    }

    /// Persist a daily watermark so restarts and new deployments cannot repeat a
    /// check or turn ordinary image drift into an immediate replacement.
    pub fn check_daily_runtime_update(
        &mut self,
        target: &str,
        at: &str,
    ) -> Result<bool, SandboxModelError> {
        if !self.supports_retained_runtime_update()
            || self.requires_retained_runtime_transition()
            || !matches!(self.status, SandboxStatus::Active | SandboxStatus::Paused)
        {
            return Ok(false);
        }
        if !valid_version(target) {
            return Err(invalid());
        }
        let now = seconds_from_rfc3339(at).ok_or_else(invalid)?;
        if self
            .lifecycle
            .runtime_update
            .as_ref()
            .is_some_and(|state| state.observed_version != target)
        {
            let state = self.lifecycle.runtime_update.as_mut().ok_or_else(invalid)?;
            state.observed_version = target.into();
            state.checked_at = at.into();
            self.touch_runtime_update(at.into())?;
            return Ok(true);
        }
        let boundary = now.saturating_add(6 * 3600) / 86400 * 86400;
        let boundary = boundary.saturating_sub(6 * 3600);
        if self.lifecycle.runtime_update.as_ref().is_some_and(|state| {
            seconds_from_rfc3339(&state.checked_at).is_some_and(|checked| checked >= boundary)
        }) {
            return Ok(false);
        }
        let initialized = self.lifecycle.runtime_update.is_some();
        let before = self.revision;
        if initialized
            && self
                .runtime_update_request()
                .is_none_or(|r| r.completed_at.is_some())
        {
            self.request_runtime_update(target.into(), true, at.into())?;
        }
        self.lifecycle
            .runtime_update
            .get_or_insert_with(|| SandboxRuntimeUpdateState {
                checked_at: at.into(),
                observed_version: target.into(),
                request: None,
            })
            .checked_at = at.into();
        if self.revision == before {
            self.touch_runtime_update(at.into())?;
        }
        Ok(true)
    }

    fn touch_runtime_update(&mut self, at: String) -> Result<(), SandboxModelError> {
        self.revision = self.revision.checked_add(1).ok_or_else(invalid)?;
        self.updated_at = at;
        self.validate()
    }

    /// A failed update retains its precise phase and storage authority for retry.
    pub fn park_failed_runtime_update(
        &mut self,
        failure: TypedFailure,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if self.admitted_runtime_update_target().is_none() {
            return Err(invalid());
        }
        self.apply_claimed_with_step(
            holder,
            fence,
            SandboxWorkStep::None,
            at.clone(),
            at,
            |next| {
                next.lifecycle
                    .runtime_update
                    .as_mut()
                    .and_then(|s| s.request.as_mut())
                    .ok_or_else(invalid)?
                    .failure = Some(failure);
                Ok(())
            },
        )
    }
}

/// Must be evaluated in the same transaction that fences every Agent.
/// Deleted publication already requires the owned worktree to be Removed;
/// retained turn and output-checkpoint fields are immutable terminal history.
pub fn runtime_update_agents_are_idle(agents: &[Agent]) -> bool {
    agents.iter().all(|agent| {
        // Deletion preserves the interrupted turn as history. Once deletion
        // and cleanup finish, that historical sequence cannot be dispatched.
        // Keep every real queue, operation, lease and cleanup fence below.
        let deleted = agent.status == AgentStatus::Deleted;
        (!agent.status.is_live()
            || matches!(agent.status, AgentStatus::Ready | AgentStatus::Paused))
            && (!deleted
                || (agent
                    .deleted_at
                    .as_deref()
                    .and_then(seconds_from_rfc3339)
                    .is_some()
                    && agent.work.cleanup_debt.is_empty()))
            && (deleted || agent.conversation.active_turn.is_none())
            && agent
                .conversation
                .operation
                .as_ref()
                .is_none_or(|op| !op.blocks_dispatch())
            && agent.conversation.queued_messages.is_empty()
            && !agent.work.step.is_pending()
            && agent.work.lease.is_none()
            && agent.requested_stop_reason.is_none()
    })
}

fn valid_version(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.runtime_update")
}
