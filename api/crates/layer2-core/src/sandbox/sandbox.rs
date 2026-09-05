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
    pub runtime_id: String,
    pub runtime_version: String,
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
        if !self.credential.validate() || self.work.next_fence == 0 {
            return Err(SandboxModelError::invalid_value(
                "sandbox.credential_or_work",
            ));
        }
        self.lifecycle.validate(self)?;
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
        let mut next = self.clone();
        next.lifecycle.cancel_operation();
        next.transition(SandboxStatus::Retiring, at.clone(), None)?;
        next.work.schedule(SandboxWorkStep::StopAgents, at);
        *self = next;
        Ok(())
    }

    pub fn begin_pause(
        &mut self,
        at: String,
        notice: Option<String>,
    ) -> Result<(), SandboxModelError> {
        if self.status != SandboxStatus::Active
            || self.work.lease.is_some()
            || !matches!(
                self.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
        {
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
        if self.status != SandboxStatus::Paused
            || self.work.lease.is_some()
            || self.work.step != SandboxWorkStep::None
        {
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

    pub fn begin_image_rotation(
        &mut self,
        target_runtime_version: String,
        identity: SandboxRotationIdentity,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if self.status != SandboxStatus::Active
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
            target_runtime_version,
            identity,
        )?;
        next.transition(SandboxStatus::Rotating, at.clone(), None)?;
        next.work
            .schedule(SandboxWorkStep::RunLifecycleOperation, at);
        *self = next;
        Ok(())
    }
}
