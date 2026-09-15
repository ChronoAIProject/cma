//! A reused ordinary parent may revive for its channel-created child only
//! under the retained configuring human's current ManageSandbox decision.
//! This proof does not authorize Channel-root credential lifecycle effects.
use super::*;
use layer2_core::{
    bridge::allocation::{AllocationError, AllocationState},
    sandbox::{Sandbox, SandboxStatus, SandboxWorkStep},
};

pub(crate) struct ChannelReuseRevivalAuthority {
    allocation: AllocationRecord,
    authority: AllocationAuthority,
    human: Arc<BridgeControlPlaneLease>,
    sandbox: Sandbox,
}

impl ChannelReuseRevivalAuthority {
    pub(crate) fn allocation(&self) -> &AllocationRecord {
        &self.allocation
    }
    pub(crate) fn allocation_authority(&self) -> &AllocationAuthority {
        &self.authority
    }
    pub(crate) fn human(&self) -> &BridgeControlPlaneLease {
        &self.human
    }
    pub(crate) fn sandbox(&self) -> &Sandbox {
        &self.sandbox
    }

    pub(crate) fn require_current(&self) -> Result<(), AllocationError> {
        self.authority.require_record(&self.allocation)?;
        let AllocationAuthority::Reuse(proof) = &self.authority else {
            return Err(AllocationError::IdentityMismatch);
        };
        let AllocationPlacement::ReuseParent { target, .. } = &self.allocation.placement else {
            return Err(AllocationError::IdentityMismatch);
        };
        if self.sandbox != proof.resources().lineage.sandbox
            || self.sandbox.status != SandboxStatus::Paused
            || self.sandbox.lifecycle.operation.is_some()
            || self.sandbox.lifecycle.checkpoint_capture.is_some()
            || self.sandbox.work.lease.is_some()
            || self.sandbox.work.execution.is_some()
            || self.sandbox.work.step != SandboxWorkStep::None
            || target.sandbox_id != self.sandbox.id
            || !matches!(
                self.allocation.state,
                AllocationState::Ready | AllocationState::Retired
            )
            || self.allocation.receipt.is_none()
            || !self.allocation.effect_started
            || self.allocation.owner != *self.human.owner()
            || self.allocation.identity.bridge_id != *self.human.bridge_id()
            || self.allocation.config.connector != *self.human.connector()
            || tokio::time::Instant::now() >= self.human.deadline()
        {
            return Err(AllocationError::AuthorityUnavailable);
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

impl BridgeProvisioningAuthority {
    pub(crate) async fn resolve_reused_revival(
        &self,
        definitions: &BridgeService,
        allocation: &AllocationRecord,
        sandbox: &Sandbox,
    ) -> Result<ChannelReuseRevivalAuthority, BridgeError> {
        let (authority, human) = self.resolve(definitions, allocation).await?;
        let permit = ChannelReuseRevivalAuthority {
            allocation: allocation.clone(),
            authority,
            human: Arc::new(human),
            sandbox: sandbox.clone(),
        };
        permit.require_current().map_err(allocation_error)?;
        Ok(permit)
    }
}
