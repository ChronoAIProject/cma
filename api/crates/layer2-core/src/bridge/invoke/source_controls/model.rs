use serde::{Deserialize, Serialize};

use super::super::{
    BridgeInvokeError as Error, BridgeInvokeGrant, BridgeInvokeGrantId, BridgeInvokeTarget,
};
use crate::bridge::BridgeWorkPin;
use crate::resource_access::{
    ResourceActor, ResourceAuthorizationAudit, ResourceCapability, ResourceOwner,
};

pub const MAX_SOURCE_CONTROL_REVISIONS: u64 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceControlAction {
    AllowSenderCancelAndSteer,
    RevokeSenderControls,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceControlRequest {
    pub expected_bridge_revision: u64,
    pub expected_grant_revision: u64,
    /// Zero means no policy has ever been admitted for this invoke grant.
    pub expected_policy_revision: u64,
    pub action: SourceControlAction,
}
impl SourceControlRequest {
    pub fn validate(&self) -> Result<(), Error> {
        if !(1..=i64::MAX as u64).contains(&self.expected_bridge_revision)
            || !(1..=2).contains(&self.expected_grant_revision)
            || self.expected_policy_revision >= MAX_SOURCE_CONTROL_REVISIONS
            || (self.action == SourceControlAction::AllowSenderCancelAndSteer
                && self.expected_policy_revision >= MAX_SOURCE_CONTROL_REVISIONS - 1)
        {
            return Err(Error::Invalid("source_control_request"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceControlScope {
    SourceSender,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceControlApprovalMode {
    DashboardOnly,
}

/// A consent revision, not proof of current permission or runtime support.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceControlPolicyView {
    pub id: String,
    pub grant_id: BridgeInvokeGrantId,
    pub owner: ResourceOwner,
    pub pin: BridgeWorkPin,
    pub target: BridgeInvokeTarget,
    pub revision: u64,
    pub enabled: bool,
    pub scope: SourceControlScope,
    pub approval_mode: SourceControlApprovalMode,
    pub updated_at: String,
    pub updated_by: ResourceActor,
}
impl SourceControlPolicyView {
    pub fn validate(&self) -> Result<(), Error> {
        self.grant_id.validate().map_err(|_| Error::Corrupt)?;
        self.target.validate()?;
        let expected = policy_id(&self.grant_id)?;
        let owner = match &self.owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        };
        if self.id != expected
            || !crate::bridge::config::valid_id(owner.as_str())
            || !(1..=MAX_SOURCE_CONTROL_REVISIONS).contains(&self.revision)
            || (self.enabled && self.revision == MAX_SOURCE_CONTROL_REVISIONS)
            || !super::super::model::actual_actor(&self.updated_by)
            || !super::super::model::stamp(&self.updated_at)
            || !crate::bridge::config::valid_id(self.pin.bridge_id.as_str())
            || !(1..=256).contains(&self.pin.config_revision)
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
    pub fn require_grant(&self, grant: &BridgeInvokeGrant) -> Result<(), Error> {
        self.validate()?;
        grant.validate()?;
        if self.grant_id != grant.id
            || self.owner != grant.owner
            || self.pin != grant.pin
            || self.target != grant.target
        {
            return Err(Error::TargetMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceControlPolicy {
    pub view: SourceControlPolicyView,
    pub receipt_id: String,
    pub request_sha256: String,
    pub resource_audit: Vec<ResourceAuthorizationAudit>,
}
impl SourceControlPolicy {
    pub fn validate(&self) -> Result<(), Error> {
        self.view.validate()?;
        if !self
            .receipt_id
            .strip_prefix("brscc_")
            .is_some_and(super::super::model::digest)
            || !super::super::model::digest(&self.request_sha256)
            || self.resource_audit.len() != 2
        {
            return Err(Error::Corrupt);
        }
        for (audit, capability) in self.resource_audit.iter().zip([
            ResourceCapability::ManageBridge,
            ResourceCapability::Interact,
        ]) {
            audit.validate().map_err(|_| Error::Corrupt)?;
            if audit.actor != self.view.updated_by
                || audit.owner != self.view.owner
                || audit.target != self.view.target.resource()
                || audit.sandbox_id != self.view.target.ids.sandbox_id
                || audit.sandbox_created_at != self.view.target.sandbox_created_at
                || audit.capability != capability
            {
                return Err(Error::Corrupt);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceControlReceipt {
    pub id: String,
    pub request: SourceControlRequest,
    pub result: SourceControlPolicyView,
}
impl SourceControlReceipt {
    pub fn validate(&self) -> Result<(), Error> {
        self.request.validate().map_err(|_| Error::Corrupt)?;
        self.result.validate()?;
        if !self
            .id
            .strip_prefix("brscc_")
            .is_some_and(super::super::model::digest)
            || self.result.revision != self.request.expected_policy_revision + 1
            || self.result.enabled
                != (self.request.action == SourceControlAction::AllowSenderCancelAndSteer)
            || (self.result.enabled && self.request.expected_grant_revision != 1)
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
    pub fn require_policy(&self, policy: &SourceControlPolicy) -> Result<(), Error> {
        self.validate()?;
        policy.validate()?;
        if self.id != policy.receipt_id
            || self.result != policy.view
            || source_control_request_digest(&self.request)? != policy.request_sha256
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
}

pub fn source_control_request_digest(request: &SourceControlRequest) -> Result<String, Error> {
    request.validate()?;
    Ok(crate::bridge::store::digest(&(
        "cma.source-control.request.v1",
        request,
    ))?)
}
pub(super) fn policy_id(grant: &BridgeInvokeGrantId) -> Result<String, Error> {
    Ok(format!(
        "brscp_{}",
        crate::bridge::store::digest(&("cma.source-control.policy.v1", grant))?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_history_reserves_its_last_revision_for_revocation() {
        let mut request = SourceControlRequest {
            expected_bridge_revision: 1,
            expected_grant_revision: 1,
            expected_policy_revision: MAX_SOURCE_CONTROL_REVISIONS - 1,
            action: SourceControlAction::AllowSenderCancelAndSteer,
        };
        assert!(
            request.validate().is_err(),
            "enablement must not consume the last revocation slot"
        );
        request.action = SourceControlAction::RevokeSenderControls;
        assert!(request.validate().is_ok());
        request.expected_policy_revision = MAX_SOURCE_CONTROL_REVISIONS;
        assert!(request.validate().is_err());
    }
}
