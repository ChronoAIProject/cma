use serde::{Deserialize, Serialize};

use crate::resource_access::{ResourceActor, ResourceAuthorizationAudit, ResourceOwner};

use super::super::{BridgeConfig, BridgeError, BridgeId, BridgeWorkPin};
use super::BridgeInvokeTarget;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeInvokeError {
    #[error(transparent)]
    Bridge(#[from] BridgeError),
    #[error("invoke grant not found")]
    NotFound,
    #[error("invoke grant management is forbidden")]
    Forbidden,
    #[error("invoke grant changed")]
    Conflict,
    #[error("invoke grant key was already used for different consent")]
    IdempotencyConflict,
    #[error("invoke grant is revoked")]
    Revoked,
    #[error("invoke target identity or ownership changed")]
    TargetMismatch,
    #[error("bridge execution is suspended")]
    Suspended,
    #[error("invalid invoke grant field: {0}")]
    Invalid(&'static str),
    #[error("invoke authority is unavailable")]
    Unavailable,
    #[error("invoke grant data is inconsistent")]
    Corrupt,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BridgeInvokeGrantId(pub String);

impl BridgeInvokeGrantId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn validate(&self) -> Result<(), BridgeInvokeError> {
        self.0
            .strip_prefix("brinv_")
            .is_some_and(digest)
            .then_some(())
            .ok_or(BridgeInvokeError::Invalid("grant_id"))
    }
}

/// Explicit request input, with no default. The retained Agent's existing
/// conversation and context will be shared with this bridge's admitted inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeSharedContextConsent {
    UseRetainedAgentContext,
    /// Derived only from verified channel allocation and immutable creation.
    UseChannelCreatedAgent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeInvokeGrantState {
    Active,
    Revoked { at: String, actor: ResourceActor },
}

/// Full immutable issuance identity plus terminal local revocation. Configuration
/// refreshes never rewrite consent or reactivate this grant. This internal record
/// includes connector references; public endpoints use BridgeInvokeGrantView.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeInvokeGrant {
    pub id: BridgeInvokeGrantId,
    pub revision: u64,
    pub owner: ResourceOwner,
    pub pin: BridgeWorkPin,
    pub config: BridgeConfig,
    pub target: BridgeInvokeTarget,
    pub consent: BridgeSharedContextConsent,
    pub issued_at: String,
    pub issued_by: ResourceActor,
    pub resource_audit: Vec<ResourceAuthorizationAudit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<Box<super::ChannelInvokeBasis>>,
    pub state: BridgeInvokeGrantState,
}

impl BridgeInvokeGrant {
    pub fn validate(&self) -> Result<(), BridgeInvokeError> {
        self.id.validate().map_err(|_| BridgeInvokeError::Corrupt)?;
        self.config
            .validate()
            .map_err(|_| BridgeInvokeError::Corrupt)?;
        self.target.validate()?;
        if !super::super::config::valid_id(self.pin.bridge_id.as_str())
            || self.pin.config_revision == 0
            || self.pin.config_revision > 256
            || !actual_actor(&self.issued_by)
            || !stamp(&self.issued_at)
            || self.resource_audit.len() != 2
        {
            return Err(BridgeInvokeError::Corrupt);
        }
        match (&self.config.placement, &self.consent, &self.channel) {
            (
                super::super::BridgePlacementPolicy::ExistingAgent { target },
                BridgeSharedContextConsent::UseRetainedAgentContext,
                None,
            ) if target == &self.target.ids => {}
            (placement, BridgeSharedContextConsent::UseChannelCreatedAgent, Some(basis))
                if placement.creates_partition_agent() =>
            {
                basis.validate(self)?
            }
            _ => return Err(BridgeInvokeError::Corrupt),
        }
        for (audit, capability) in self.resource_audit.iter().zip([
            crate::resource_access::ResourceCapability::ManageBridge,
            crate::resource_access::ResourceCapability::Interact,
        ]) {
            audit.validate().map_err(|_| BridgeInvokeError::Corrupt)?;
            if audit.actor != self.issued_by
                || audit.owner != self.owner
                || audit.target != self.target.resource()
                || audit.sandbox_id != self.target.ids.sandbox_id
                || audit.sandbox_created_at != self.target.sandbox_created_at
                || audit.capability != capability
            {
                return Err(BridgeInvokeError::Corrupt);
            }
        }
        match (&self.state, self.revision) {
            (BridgeInvokeGrantState::Active, 1) => Ok(()),
            (BridgeInvokeGrantState::Revoked { at, actor }, 2)
                if stamp(at) && actual_actor(actor) =>
            {
                Ok(())
            }
            _ => Err(BridgeInvokeError::Corrupt),
        }
    }

    pub fn require_active(&self) -> Result<(), BridgeInvokeError> {
        self.validate()?;
        if self.state != BridgeInvokeGrantState::Active {
            return Err(BridgeInvokeError::Revoked);
        }
        Ok(())
    }

    pub fn view(&self) -> BridgeInvokeGrantView {
        BridgeInvokeGrantView {
            id: self.id.clone(),
            revision: self.revision,
            owner: self.owner.clone(),
            pin: self.pin.clone(),
            target: self.target.clone(),
            consent: self.consent.clone(),
            issued_at: self.issued_at.clone(),
            issued_by: self.issued_by.clone(),
            state: self.state.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeInvokeGrantView {
    pub id: BridgeInvokeGrantId,
    pub revision: u64,
    pub owner: ResourceOwner,
    pub pin: BridgeWorkPin,
    pub target: BridgeInvokeTarget,
    pub consent: BridgeSharedContextConsent,
    pub issued_at: String,
    pub issued_by: ResourceActor,
    pub state: BridgeInvokeGrantState,
}

impl BridgeInvokeGrantView {
    pub fn validate(&self) -> Result<(), BridgeInvokeError> {
        self.id.validate().map_err(|_| BridgeInvokeError::Corrupt)?;
        self.target.validate()?;
        let owner = match &self.owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        };
        if !super::super::config::valid_id(owner.as_str())
            || !super::super::config::valid_id(self.pin.bridge_id.as_str())
            || !(1..=256).contains(&self.pin.config_revision)
            || !actual_actor(&self.issued_by)
            || !stamp(&self.issued_at)
        {
            return Err(BridgeInvokeError::Corrupt);
        }
        match (&self.state, self.revision) {
            (BridgeInvokeGrantState::Active, 1) => Ok(()),
            (BridgeInvokeGrantState::Revoked { at, actor }, 2)
                if stamp(at) && actual_actor(actor) =>
            {
                Ok(())
            }
            _ => Err(BridgeInvokeError::Corrupt),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeInvokeReceipt {
    pub id: String,
    pub request_digest: String,
    pub bridge_id: BridgeId,
    pub actor: ResourceActor,
    pub result: BridgeInvokeGrantView,
}

impl BridgeInvokeReceipt {
    pub fn validate(&self) -> Result<(), BridgeInvokeError> {
        self.result.validate()?;
        if !digest(&self.id)
            || !digest(&self.request_digest)
            || !actual_actor(&self.actor)
            || self.bridge_id != self.result.pin.bridge_id
            || match &self.result.state {
                BridgeInvokeGrantState::Active => self.actor != self.result.issued_by,
                BridgeInvokeGrantState::Revoked { actor, .. } => &self.actor != actor,
            }
        {
            return Err(BridgeInvokeError::Corrupt);
        }
        Ok(())
    }
}

pub(super) fn actual_actor(actor: &ResourceActor) -> bool {
    matches!(actor, ResourceActor::Human {subject} | ResourceActor::Api {subject,delegation_id:None}
        if super::super::config::valid_id(subject.as_str()))
}
pub(super) fn stamp(value: &str) -> bool {
    crate::seconds_from_rfc3339(value).is_some()
}
pub(super) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
