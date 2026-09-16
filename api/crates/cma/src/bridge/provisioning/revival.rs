//! Original-human authority for a concrete paused or reviving Channel resource.
//! This never grants cleanup authority or broadens active credential renewal.
use super::*;
use crate::store::MongoBridgeAllocationStore;
use layer2_core::sandbox::{Sandbox, SandboxLifecycleOperation, SandboxStatus};

pub(crate) struct ChannelRevivalAuthority {
    allocation: AllocationRecord,
    authority: AllocationAuthority,
    human: Arc<BridgeControlPlaneLease>,
    sandbox: Sandbox,
}
impl ChannelRevivalAuthority {
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
    pub(crate) fn after_credential_intent(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Self, layer2_core::bridge::allocation::AllocationError> {
        let mut expected = self.sandbox.clone();
        expected.revision = expected
            .revision
            .checked_add(1)
            .ok_or(layer2_core::bridge::allocation::AllocationError::Corrupt)?;
        expected.updated_at = sandbox.updated_at.clone();
        if expected != *sandbox {
            return Err(layer2_core::bridge::allocation::AllocationError::IdentityMismatch);
        }
        Ok(Self {
            allocation: self.allocation.clone(),
            authority: self.authority.clone(),
            human: self.human.clone(),
            sandbox: sandbox.clone(),
        })
    }
    pub fn require_current(&self) -> Result<(), layer2_core::bridge::allocation::AllocationError> {
        use layer2_core::bridge::allocation::{AllocationError, AllocationState};
        self.authority.require_record(&self.allocation)?;
        let eligible = match self.sandbox.status {
            SandboxStatus::Paused => {
                self.sandbox.lifecycle.operation.is_none()
                    && self.sandbox.work.lease.is_none()
                    && self.sandbox.work.step == layer2_core::sandbox::SandboxWorkStep::None
            }
            SandboxStatus::Reviving => {
                matches!(
                    self.sandbox.lifecycle.operation,
                    Some(SandboxLifecycleOperation::Revive { .. })
                ) && self.sandbox.work.step
                    == layer2_core::sandbox::SandboxWorkStep::RunLifecycleOperation
                    && self.sandbox.work.lease.as_ref().is_some_and(|claim| {
                        claim.fence > 0
                            && !claim.holder.is_empty()
                            && mongodb::bson::DateTime::parse_rfc3339_str(&claim.expires_at)
                                .is_ok_and(|expiry| {
                                    expiry.to_system_time() > std::time::SystemTime::now()
                                })
                    })
            }
            _ => false,
        };
        if !eligible
            || !matches!(
                self.allocation.state,
                AllocationState::Ready | AllocationState::Retired
            )
            || self.allocation.receipt.is_none()
            || !self.allocation.effect_started
            || self.allocation.placement.target().sandbox_id != self.sandbox.id
            || match &self.allocation.owner {
                ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => {
                    subject != &self.sandbox.owner
                }
            }
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
    pub(crate) async fn resolve_revival(
        &self,
        definitions: &BridgeService,
        allocations: &MongoBridgeAllocationStore,
        allocation: &AllocationRecord,
        sandbox: &Sandbox,
    ) -> Result<ChannelRevivalAuthority, BridgeError> {
        let (authority, human) = self.resolve(definitions, allocation).await?;
        let permit = ChannelRevivalAuthority {
            allocation: allocation.clone(),
            authority,
            human: Arc::new(human),
            sandbox: sandbox.clone(),
        };
        permit.require_current().map_err(allocation_error)?;
        allocations
            .current_channel_lifecycle(&permit)
            .await
            .map_err(allocation_error)?;
        Ok(permit)
    }
}

#[async_trait::async_trait]
pub(crate) trait ChannelRevivalAuthoritySource: Send + Sync {
    async fn resolve(&self, sandbox: &Sandbox) -> Result<ChannelRevivalAuthority, BridgeError>;
    async fn resolve_reused(
        &self,
        _allocation: &AllocationRecord,
        _sandbox: &Sandbox,
    ) -> Result<super::reuse_revival::ChannelReuseRevivalAuthority, BridgeError> {
        Err(BridgeError::ConnectionRequired(
            BridgeSetupBlock::IncompatibleTarget,
        ))
    }
}
pub(crate) struct ManagedChannelRevivalAuthority {
    pub definitions: BridgeService,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub authority: Arc<BridgeProvisioningAuthority>,
}
#[async_trait::async_trait]
impl ChannelRevivalAuthoritySource for ManagedChannelRevivalAuthority {
    async fn resolve_reused(
        &self,
        allocation: &AllocationRecord,
        sandbox: &Sandbox,
    ) -> Result<super::reuse_revival::ChannelReuseRevivalAuthority, BridgeError> {
        self.authority
            .resolve_reused_revival(&self.definitions, allocation, sandbox)
            .await
    }

    async fn resolve(&self, sandbox: &Sandbox) -> Result<ChannelRevivalAuthority, BridgeError> {
        let allocation = self
            .allocations
            .channel_lifecycle_origin(&sandbox.id)
            .await
            .map_err(allocation_error)?;
        self.authority
            .resolve_revival(&self.definitions, &self.allocations, &allocation, sandbox)
            .await
    }
}
