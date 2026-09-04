use serde::{Deserialize, Serialize};

use crate::ids::{SandboxId, ServiceId, SubjectId};
use crate::profiles::SandboxProfileSnapshot;

use super::{SandboxModelError, SandboxStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBinding {
    pub runtime_id: String,
    pub runtime_version: String,
    pub bound_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCredentialState {
    #[serde(default)]
    pub reaches: Vec<ServiceId>,
    pub key_generation: u64,
    pub delivered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxLimits {
    pub max_workspaces: u8,
    pub max_agents_per_workspace: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCheckpointRef {
    pub object_reference: String,
    pub sha256: String,
    pub sealed_at: String,
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
    pub profile: SandboxProfileSnapshot,
    pub status: SandboxStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<TypedFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeBinding>,
    pub credential: SandboxCredentialState,
    pub limits: SandboxLimits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<SandboxCheckpointRef>,
    pub last_activity_at: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
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
        self.status = self.status.transition(next)?;
        self.failure = failure;
        self.updated_at = at.clone();
        if next == SandboxStatus::Retired {
            self.retired_at = Some(at);
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("sandbox.revision"))?;
        Ok(())
    }
}
