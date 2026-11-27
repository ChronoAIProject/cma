//! Pin managed software when a user-admitted resume retains the existing home.
use super::{Sandbox, SandboxEventKind, SandboxModelError, SandboxReviveCause, SandboxStatus};

impl Sandbox {
    /// The target and revival are one model transition and one storage revision.
    /// A prior unfinished update keeps its accepted target across deployments.
    pub fn begin_revive_with_managed_runtime(
        &mut self,
        cause: SandboxReviveCause,
        target: Option<&str>,
        at: String,
    ) -> Result<(), SandboxModelError> {
        let mut next = self.clone();
        if next.status == SandboxStatus::Paused
            && cause != SandboxReviveCause::RuntimeLost
            && next.supports_retained_runtime_update()
            && !next.requires_retained_runtime_transition()
        {
            let target = next
                .runtime_update_request()
                .filter(|request| request.completed_at.is_none())
                .map(|request| request.target_version.clone())
                .or_else(|| target.map(str::to_owned));
            if let Some(target) = target {
                next.request_runtime_update(target, true, at.clone())?;
                // request_runtime_update normally commits alone; here its data
                // is admitted atomically by the following lifecycle transition.
                next.revision = self.revision;
                if let Some(request) = next
                    .lifecycle
                    .runtime_update
                    .as_mut()
                    .and_then(|state| state.request.as_mut())
                    .filter(|request| request.completed_at.is_none())
                {
                    request.started_at.get_or_insert_with(|| at.clone());
                }
            }
        }
        next.begin_revive(cause, at)?;
        *self = next;
        Ok(())
    }

    /// Child admission uses the same pinned resume target in its parent CAS.
    pub fn record_creation_admission_with_managed_runtime(
        &mut self,
        cause: SandboxReviveCause,
        target: Option<&str>,
        at: String,
    ) -> Result<Option<SandboxEventKind>, SandboxModelError> {
        if self.status != SandboxStatus::Paused {
            return self.record_creation_admission(cause, at);
        }
        if !matches!(
            cause,
            SandboxReviveCause::WorkspaceCreation
                | SandboxReviveCause::AgentCreation
                | SandboxReviveCause::TriggerLaunch
        ) {
            return Err(SandboxModelError::invalid_value(
                "sandbox.creation_admission",
            ));
        }
        let mut next = self.clone();
        next.begin_revive_with_managed_runtime(cause, target, at.clone())?;
        next.last_activity_at = at;
        next.validate()?;
        *self = next;
        Ok(Some(SandboxEventKind::StatusChanged {
            from: SandboxStatus::Paused,
            to: SandboxStatus::Reviving,
        }))
    }
}
