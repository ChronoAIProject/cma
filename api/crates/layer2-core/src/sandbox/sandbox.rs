use serde::{Deserialize, Serialize};

use crate::ids::{SandboxId, SubjectId};
use crate::profiles::SandboxProfileSnapshot;

use super::{
    SandboxCheckpointRef, SandboxCredentialState, SandboxModelError, SandboxOrigin, SandboxStatus,
    SandboxWorkState, SandboxWorkStep,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<SandboxCheckpointRef>,
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
        if self
            .checkpoint
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.validate().is_err())
        {
            return Err(SandboxModelError::invalid_value("sandbox.checkpoint"));
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
        self.transition(SandboxStatus::Retiring, at.clone(), None)?;
        self.work.schedule(SandboxWorkStep::StopAgents, at);
        Ok(())
    }
}
