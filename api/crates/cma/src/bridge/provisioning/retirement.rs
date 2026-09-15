//! Cleanup authority belongs to the original human and the exact created root.
use super::*;
use crate::store::MongoBridgeAllocationStore;
use layer2_core::{
    bridge::allocation::{AllocationError, AllocationState},
    sandbox::{Sandbox, SandboxStatus, SandboxWorkStep},
};

pub(crate) struct ChannelRetirementAuthority {
    allocation: AllocationRecord,
    authority: AllocationAuthority,
    human: Arc<BridgeControlPlaneLease>,
    sandbox: Sandbox,
}
impl ChannelRetirementAuthority {
    pub fn allocation(&self) -> &AllocationRecord {
        &self.allocation
    }
    pub fn allocation_authority(&self) -> &AllocationAuthority {
        &self.authority
    }
    pub fn human(&self) -> &BridgeControlPlaneLease {
        &self.human
    }
    pub fn sandbox(&self) -> &Sandbox {
        &self.sandbox
    }
    pub fn require_current(&self) -> Result<(), AllocationError> {
        self.authority.require_record(&self.allocation)?;
        // A reused Workspace/parent never becomes a disposable Channel root.
        if !matches!(
            self.allocation.placement,
            AllocationPlacement::NewSandbox { .. }
        ) || !matches!(
            self.allocation.state,
            AllocationState::Ready | AllocationState::Retired
        ) || self.allocation.receipt.is_none()
            || !self.allocation.effect_started
            || self.sandbox.status != SandboxStatus::Retiring
            || !matches!(
                self.sandbox.work.step,
                SandboxWorkStep::StopAgents
                    | SandboxWorkStep::RevokeRetirementCredentials
                    | SandboxWorkStep::TeardownRuntime
                    | SandboxWorkStep::MarkRetired
            )
            || self.sandbox.lifecycle.operation.is_some()
            || self.sandbox.lifecycle.checkpoint_capture.is_some()
            || self.sandbox.lifecycle.background_checkpoint.is_some()
            || self.allocation.placement.target().sandbox_id != self.sandbox.id
            || self.allocation.owner != *self.human.owner()
            || self.allocation.identity.bridge_id != *self.human.bridge_id()
            || self.allocation.config.connector != *self.human.connector()
            || tokio::time::Instant::now() >= self.human.deadline()
        {
            return Err(AllocationError::AuthorityUnavailable);
        }
        let claim = self
            .sandbox
            .work
            .lease
            .as_ref()
            .ok_or(AllocationError::LeaseUnavailable)?;
        if claim.fence == 0
            || claim.holder.is_empty()
            || !mongodb::bson::DateTime::parse_rfc3339_str(&claim.expires_at)
                .is_ok_and(|expiry| expiry.to_system_time() > std::time::SystemTime::now())
        {
            return Err(AllocationError::LeaseUnavailable);
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn fixture(
        allocation: AllocationRecord,
        authority: AllocationAuthority,
        human: Arc<BridgeControlPlaneLease>,
        sandbox: Sandbox,
    ) -> Self {
        Self {
            allocation,
            authority,
            human,
            sandbox,
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait ChannelRetirementAuthoritySource: Send + Sync {
    async fn resolve(&self, sandbox: &Sandbox) -> Result<ChannelRetirementAuthority, BridgeError>;
}
pub(crate) struct ManagedChannelRetirementAuthority {
    pub definitions: BridgeService,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub authority: Arc<BridgeProvisioningAuthority>,
}
#[async_trait::async_trait]
impl ChannelRetirementAuthoritySource for ManagedChannelRetirementAuthority {
    async fn resolve(&self, sandbox: &Sandbox) -> Result<ChannelRetirementAuthority, BridgeError> {
        let allocation = self
            .allocations
            .channel_lifecycle_origin(&sandbox.id)
            .await
            .map_err(allocation_error)?;
        let (authority, human) = self
            .authority
            .resolve(&self.definitions, &allocation)
            .await?;
        let permit = ChannelRetirementAuthority {
            allocation,
            authority,
            human: Arc::new(human),
            sandbox: sandbox.clone(),
        };
        permit.require_current().map_err(allocation_error)?;
        Ok(permit)
    }
}
