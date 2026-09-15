use serde::{Deserialize, Serialize};

use crate::bridge::{partition::PartitionSourcePin, BridgeWorkPin};
use crate::resource_access::{ResourceActor, ResourceOwner};
use crate::sandbox::{Sandbox, SandboxStatus, SandboxWorkStep};

use super::super::{AllocationError, AllocationPlacement, AllocationRecord};

/// Immutable creation facts live outside the legacy Sandbox record. The hold
/// cannot be released by generic lifecycle work or a caller-supplied actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelSandboxSidecar {
    pub schema_version: u16,
    pub allocation_id: String,
    pub owner: ResourceOwner,
    pub pin: BridgeWorkPin,
    pub source: PartitionSourcePin,
    pub sandbox_id: crate::SandboxId,
    pub sandbox_created_at: String,
    pub profiles_digest: String,
    pub approved_by: ResourceActor,
    pub initial_fingerprint: String,
    pub hold: ChannelSandboxHold,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelSandboxHold {
    RenewableAuthorityUnavailable,
}

impl ChannelSandboxSidecar {
    pub(super) fn capture(
        allocation: &AllocationRecord,
        sandbox: &Sandbox,
    ) -> Result<Self, AllocationError> {
        let AllocationPlacement::NewSandbox { profiles, target } = &allocation.placement else {
            return Err(AllocationError::IdentityMismatch);
        };
        let subject = match &allocation.owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        };
        if sandbox.id != target.sandbox_id
            || &sandbox.owner != subject
            || sandbox.profile != profiles.sandbox
        {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(Self {
            schema_version: 1,
            allocation_id: allocation.id.clone(),
            owner: allocation.owner.clone(),
            pin: allocation.pin.clone(),
            source: allocation.first_source.clone(),
            sandbox_id: sandbox.id.clone(),
            sandbox_created_at: sandbox.created_at.clone(),
            profiles_digest: profiles.trust_digest.clone(),
            approved_by: profiles.approved_by.clone(),
            initial_fingerprint: super::super::hash(&(
                "cma.channel.sandbox-admission.v1",
                &allocation.id,
                &sandbox.id,
                &sandbox.owner,
                &sandbox.name,
                &sandbox.origin,
                &sandbox.profile,
                &sandbox.credential.reaches,
                sandbox.limits,
                &sandbox.created_at,
            ))?,
            hold: ChannelSandboxHold::RenewableAuthorityUnavailable,
        })
    }

    pub fn require_held(
        &self,
        allocation: &AllocationRecord,
        sandbox: &Sandbox,
    ) -> Result<(), AllocationError> {
        self.require_creation(allocation, sandbox)?;
        if sandbox.revision != 1
            || sandbox.status != SandboxStatus::Provisioning
            || sandbox.work.step != SandboxWorkStep::None
            || sandbox.work.lease.is_some()
            || sandbox.work.execution.is_some()
            || sandbox.runtime.is_some()
            || sandbox.credential.key_generation != 0
            || sandbox.credential.current.is_some()
            || sandbox.credential.pending.is_some()
            || sandbox.credential.delivered
            || !sandbox.credential.service_grants.is_empty()
            || !sandbox.credential.revoke_debt.is_empty()
            || sandbox.updated_at != sandbox.created_at
            || sandbox.last_activity_at != sandbox.created_at
            || sandbox.lifecycle != Default::default()
        {
            return Err(AllocationError::Corrupt);
        }
        Ok(())
    }

    /// Validate immutable origin after genuine provider/lifecycle commits.
    /// This establishes creation identity only, never permission to run work.
    pub fn require_creation(
        &self,
        allocation: &AllocationRecord,
        sandbox: &Sandbox,
    ) -> Result<(), AllocationError> {
        allocation.validate()?;
        sandbox.validate().map_err(|_| AllocationError::Corrupt)?;
        if self != &Self::capture(allocation, sandbox)? {
            return Err(AllocationError::Corrupt);
        }
        Ok(())
    }
}
