use super::super::{BridgeInvokeError as Error, BridgeInvokeGrant, BridgeInvokeTarget};
use super::{SourceControlAction, SourceControlPolicy, SourceControlReceipt, SourceControlRequest};
use crate::bridge::BridgeManagementLease;
use crate::resource_access::{AuthorizedResource, ResourceAccessSnapshot, ResourceCapability};

/// A bounded current management and exact-target observation. No public
/// constructor or deserialization can turn retained JSON into authority.
pub struct SourceControlAccess {
    pub(super) management: BridgeManagementLease,
    pub(super) target: AuthorizedResource,
    pub(super) grant: BridgeInvokeGrant,
}
impl SourceControlAccess {
    pub fn management(&self) -> &BridgeManagementLease {
        &self.management
    }
    pub fn target(&self) -> &ResourceAccessSnapshot {
        &self.target.snapshot
    }
    pub fn grant(&self) -> &BridgeInvokeGrant {
        &self.grant
    }
    pub fn require_current(&self) -> Result<(), Error> {
        self.management.require_current()?;
        self.grant.validate()?;
        for capability in [
            ResourceCapability::ManageBridge,
            ResourceCapability::Interact,
        ] {
            self.target
                .decision
                .require(capability)
                .map_err(|_| Error::Forbidden)?;
        }
        let owner = self
            .target
            .snapshot
            .scope(&self.grant.target.resource())
            .map_err(super::super::service::resource_store_error)?
            .owner;
        if self.grant.pin.bridge_id != self.management.record().id
            || self.grant.owner != self.management.record().owner
            || self.grant.owner != owner
            || BridgeInvokeTarget::capture(&self.target.snapshot)? != self.grant.target
        {
            return Err(Error::TargetMismatch);
        }
        Ok(())
    }
    pub fn require_policy(&self, policy: &SourceControlPolicy) -> Result<(), Error> {
        self.require_current()?;
        policy.validate()?;
        policy.view.require_grant(&self.grant)
    }
    pub fn require_receipt(&self, receipt: &SourceControlReceipt) -> Result<(), Error> {
        self.require_current()?;
        receipt.validate()?;
        receipt.result.require_grant(&self.grant)?;
        if receipt.result.updated_by != *self.management.actor() {
            return Err(Error::IdempotencyConflict);
        }
        Ok(())
    }
}

pub struct SourceControlWrite {
    pub(super) access: SourceControlAccess,
    pub(super) previous: Option<SourceControlPolicy>,
    pub(super) policy: SourceControlPolicy,
    pub(super) receipt: SourceControlReceipt,
}
impl SourceControlWrite {
    pub fn access(&self) -> &SourceControlAccess {
        &self.access
    }
    pub fn previous(&self) -> Option<&SourceControlPolicy> {
        self.previous.as_ref()
    }
    pub fn policy(&self) -> &SourceControlPolicy {
        &self.policy
    }
    pub fn receipt(&self) -> &SourceControlReceipt {
        &self.receipt
    }
    pub fn request(&self) -> &SourceControlRequest {
        &self.receipt.request
    }
    pub fn require_current(&self) -> Result<(), Error> {
        self.access.require_policy(&self.policy)?;
        self.access.require_receipt(&self.receipt)?;
        self.receipt.require_policy(&self.policy)?;
        let request = self.request();
        if self.access.management.record().revision != request.expected_bridge_revision
            || self.access.grant.revision != request.expected_grant_revision
            || self.previous.as_ref().map_or(0, |p| p.view.revision)
                != request.expected_policy_revision
        {
            return Err(Error::Conflict);
        }
        if let Some(previous) = &self.previous {
            self.access.require_policy(previous)?;
        }
        if request.action == SourceControlAction::AllowSenderCancelAndSteer {
            self.access.grant.require_active()?;
            if self.access.management.record().admission_pin()? != self.access.grant.pin {
                return Err(Error::Suspended);
            }
        } else if self.previous.as_ref().is_none_or(|p| !p.view.enabled) {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
