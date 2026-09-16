//! Original home requests are committed before prepare; observation is not provenance.
use super::state::{
    ChannelProvisioningPhase, ChannelProvisioningSettings, ChannelProvisioningSnapshot,
};
use layer2_core::{
    bridge::allocation::{AllocationError, AllocationRecord, ChannelSandboxSidecar},
    resource_access::{ResourceActor, ResourceOwner},
    sandbox::{Sandbox, SandboxHome},
    SandboxId,
};
use layer3_adapter::{HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeScope};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelHomeProvenance {
    pub schema_version: u16,
    pub allocation_id: String,
    pub sandbox_id: SandboxId,
    pub sandbox_created_at: String,
    pub owner: ResourceOwner,
    pub creation_approver: ResourceActor,
    pub scope: HomeVolumeScope,
    pub request: HomeVolumeRequest,
    pub first_receipt: Option<HomeVolumeReceipt>,
}

impl ChannelHomeProvenance {
    pub fn reserve(
        snapshot: &ChannelProvisioningSnapshot,
        home: &SandboxHome,
        scope: HomeVolumeScope,
    ) -> Result<Self, AllocationError> {
        if snapshot.home_provenance.is_some()
            || snapshot.state.phase != ChannelProvisioningPhase::Home
            || snapshot.state.revision != 1
            || snapshot.sandbox.lifecycle.home.is_some()
            || home.receipt.is_some()
            || home.released
        {
            return Err(AllocationError::InvalidTransition);
        }
        scope
            .validate()
            .map_err(|_| AllocationError::IdentityMismatch)?;
        home.validate(&snapshot.sandbox.id)
            .map_err(|_| AllocationError::IdentityMismatch)?;
        if snapshot.state.settings.home_policy.as_ref() != Some(&home.request.policy) {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(Self {
            schema_version: 1,
            allocation_id: snapshot.allocation.id.clone(),
            sandbox_id: snapshot.sandbox.id.clone(),
            sandbox_created_at: snapshot.sandbox.created_at.clone(),
            owner: snapshot.allocation.owner.clone(),
            creation_approver: snapshot.creation.approved_by.clone(),
            scope,
            request: home.request.clone(),
            first_receipt: None,
        })
    }

    pub fn bind(&self, receipt: HomeVolumeReceipt) -> Result<Self, AllocationError> {
        receipt
            .validate_for(&self.request)
            .map_err(|_| AllocationError::IdentityMismatch)?;
        if self
            .first_receipt
            .as_ref()
            .is_some_and(|first| first != &receipt)
        {
            return Err(AllocationError::IdentityMismatch);
        }
        let mut next = self.clone();
        next.first_receipt = Some(receipt);
        Ok(next)
    }

    pub fn validate(&self, snapshot: &ChannelProvisioningSnapshot) -> Result<(), AllocationError> {
        self.validate_retained(
            &snapshot.allocation,
            &snapshot.sandbox,
            &snapshot.creation,
            &snapshot.state.settings,
        )
    }

    pub fn validate_retained(
        &self,
        allocation: &AllocationRecord,
        sandbox: &Sandbox,
        creation: &ChannelSandboxSidecar,
        settings: &ChannelProvisioningSettings,
    ) -> Result<(), AllocationError> {
        self.scope
            .validate()
            .map_err(|_| AllocationError::IdentityMismatch)?;
        self.request
            .validate()
            .map_err(|_| AllocationError::IdentityMismatch)?;
        let home = sandbox
            .lifecycle
            .home
            .as_ref()
            .ok_or(AllocationError::IdentityMismatch)?;
        if self.schema_version != 1
            || self.allocation_id != allocation.id
            || self.sandbox_id != sandbox.id
            || self.sandbox_created_at != sandbox.created_at
            || self.owner != allocation.owner
            || self.creation_approver != creation.approved_by
            || self.request != home.request
            || home.released
            || settings.home_policy.as_ref() != Some(&self.request.policy)
        {
            return Err(AllocationError::IdentityMismatch);
        }
        if let Some(receipt) = &self.first_receipt {
            receipt
                .validate_for(&self.request)
                .map_err(|_| AllocationError::IdentityMismatch)?;
            if home.receipt.as_ref() != Some(receipt) {
                return Err(AllocationError::IdentityMismatch);
            }
        }
        // Old workers may advance the canonical home while preserving this
        // separate sidecar. An unbound receipt never becomes deletion evidence.
        Ok(())
    }

    pub fn require_successor(
        previous: &ChannelProvisioningSnapshot,
        next: &ChannelProvisioningSnapshot,
    ) -> Result<(), AllocationError> {
        match (&previous.home_provenance, &next.home_provenance) {
            (None, None) => Ok(()),
            (None, Some(value)) => {
                let home = next
                    .sandbox
                    .lifecycle
                    .home
                    .as_ref()
                    .ok_or(AllocationError::IdentityMismatch)?;
                if *value != Self::reserve(previous, home, value.scope.clone())? {
                    return Err(AllocationError::IdentityMismatch);
                }
                Ok(())
            }
            (Some(before), Some(after)) if before == after => Ok(()),
            (Some(before), Some(after))
                if previous.state.phase == ChannelProvisioningPhase::Home
                    && next.state.phase == ChannelProvisioningPhase::Home
                    && before.first_receipt.is_none() =>
            {
                let receipt = after
                    .first_receipt
                    .clone()
                    .ok_or(AllocationError::IdentityMismatch)?;
                if before.bind(receipt)? != *after {
                    return Err(AllocationError::IdentityMismatch);
                }
                Ok(())
            }
            _ => Err(AllocationError::IdentityMismatch),
        }
    }
}
