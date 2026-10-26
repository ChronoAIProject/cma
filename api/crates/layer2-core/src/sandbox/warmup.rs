//! Credential-free runtime reservations, transferred atomically at admission.

use serde::{Deserialize, Serialize};

use crate::profiles::{content_digest, SandboxProfileSnapshot};
use crate::{seconds_from_rfc3339, SandboxId, ServiceId};

use super::store::SandboxStoreError;
use super::{ProvisioningExecution, RuntimeBinding, Sandbox, SandboxStatus, SandboxWorkStep};

mod hierarchy;
pub use hierarchy::{WarmAgent, WarmWorkspace, WarmWorkspacePlan};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarmSandboxPlan {
    pub profile: SandboxProfileSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WarmWorkspacePlan>,
    pub reaches: Vec<ServiceId>,
    pub runtime_version: String,
    pub execution: ProvisioningExecution,
}

impl WarmSandboxPlan {
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        self.execution
            .validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        self.validate_hierarchy()?;
        if self.profile.revision == 0
            || self.profile.digest
                != content_digest(&self.profile.content)
                    .map_err(|error| corrupt(&error.to_string()))?
            || self.runtime_version.trim().is_empty()
            || self.reaches.windows(2).any(|pair| pair[0] >= pair[1])
            || self
                .profile
                .content
                .nyxid_connection_services()
                .iter()
                .any(|id| !self.reaches.contains(id))
        {
            return Err(corrupt("invalid environment plan"));
        }
        Ok(())
    }

    pub fn matches(&self, sandbox: &Sandbox, runtime_version: &str) -> bool {
        let mut reaches = sandbox.credential.reaches.clone();
        reaches.sort();
        reaches.dedup();
        self.profile == sandbox.profile
            && self.reaches == reaches
            && self.runtime_version == runtime_version
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarmSandboxState {
    Preparing,
    Installing,
    Ready,
    Claimed,
    Retiring,
    Retired,
}

impl WarmSandboxState {
    pub fn owns_runtime(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Installing | Self::Ready | Self::Retiring
        )
    }

    pub fn counts_toward_target(self) -> bool {
        matches!(self, Self::Preparing | Self::Installing | Self::Ready)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarmSandbox {
    /// This becomes the owned Sandbox ID; runtime identity never changes owners in place.
    pub id: SandboxId,
    pub plan: WarmSandboxPlan,
    pub state: WarmSandboxState,
    pub runtime: Option<RuntimeBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<Box<super::SandboxHome>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WarmWorkspace>,
    /// Only a validated complete install receipt may populate this field.
    pub installed_request_sha256: Option<String>,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

impl WarmSandbox {
    pub fn new(plan: WarmSandboxPlan, at: String) -> Result<Self, SandboxStoreError> {
        let record = Self {
            id: SandboxId::generate(),
            workspaces: plan.workspaces.iter().map(WarmWorkspace::new).collect(),
            plan,
            state: WarmSandboxState::Preparing,
            runtime: None,
            home: None,
            installed_request_sha256: None,
            revision: 1,
            created_at: at.clone(),
            updated_at: at,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        self.plan.validate()?;
        self.validate_hierarchy()?;
        if let Some(home) = &self.home {
            home.validate(&self.id)
                .map_err(|error| corrupt(&error.to_string()))?;
            if (self.runtime.is_some()
                && self.state != WarmSandboxState::Retired
                && home.attachment().is_none())
                || (home.released
                    && !matches!(
                        self.state,
                        WarmSandboxState::Retiring | WarmSandboxState::Retired
                    ))
                || (self.state == WarmSandboxState::Retired && !home.released)
            {
                return Err(corrupt("invalid home authority"));
            }
        }
        if self.revision == 0
            || self.id.as_str().is_empty()
            || seconds_from_rfc3339(&self.created_at).is_none()
            || seconds_from_rfc3339(&self.updated_at).is_none()
            || self.updated_at < self.created_at
            || self.runtime.as_ref().is_some_and(|binding| {
                !binding.valid_boot_id()
                    || (binding.boot_id.is_some()
                        && self.state != WarmSandboxState::Retired
                        && self
                            .home
                            .as_ref()
                            .and_then(|home| home.attachment())
                            .is_none())
                    || binding.provision_generation != Some(1)
                    || binding.runtime_version != self.plan.runtime_version
                    || binding.runtime_id.trim().is_empty()
                    || seconds_from_rfc3339(&binding.bound_at).is_none()
            })
            || (matches!(
                self.state,
                WarmSandboxState::Installing | WarmSandboxState::Ready | WarmSandboxState::Claimed
            ) && self.runtime.is_none())
            || (matches!(
                self.state,
                WarmSandboxState::Ready | WarmSandboxState::Claimed
            ) && self
                .installed_request_sha256
                .as_ref()
                .is_none_or(|digest| !super::store::valid_admission_fingerprint(digest)))
        {
            return Err(corrupt("invalid reservation"));
        }
        Ok(())
    }

    /// The store must commit this change and the owned aggregate in one transaction.
    pub fn claim(
        &mut self,
        sandbox: &mut Sandbox,
        runtime_version: &str,
        at: &str,
    ) -> Result<(), SandboxStoreError> {
        self.validate()?;
        if self.state != WarmSandboxState::Ready
            || !self.plan.has_complete_hierarchy()
            || !self.plan.matches(sandbox, runtime_version)
            || sandbox.revision != 1
            || sandbox.runtime.is_some()
            || sandbox.lifecycle.home.is_some()
            || sandbox.status != SandboxStatus::Provisioning
            || sandbox.work.step != SandboxWorkStep::ProvisionRuntime
            || sandbox.credential.key_generation != 0
            || sandbox.credential.delivered
            || sandbox.credential.current.is_some()
            || sandbox.credential.pending.is_some()
            || !sandbox.credential.service_grants.is_empty()
            || sandbox.credential.service_scope.is_some()
            || seconds_from_rfc3339(at).is_none()
            || at < self.updated_at.as_str()
        {
            return Err(SandboxStoreError::Conflict);
        }
        let mut next = sandbox.clone();
        next.id = self.id.clone();
        next.runtime = self.runtime.clone();
        next.lifecycle.home = self.home.clone();
        next.runtime
            .as_mut()
            .expect("ready runtime was validated")
            .bound_at = at.into();
        next.lifecycle.runtime_generation_high_water = 1;
        next.work.execution = Some(self.plan.execution);
        next.validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        self.state = WarmSandboxState::Claimed;
        self.updated_at = at.into();
        *sandbox = next;
        Ok(())
    }
}

fn corrupt(detail: &str) -> SandboxStoreError {
    SandboxStoreError::Corrupt(format!("warm Sandbox {detail}"))
}
