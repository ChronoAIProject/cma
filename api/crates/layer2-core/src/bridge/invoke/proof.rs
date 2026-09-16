use serde::{Deserialize, Serialize};

use crate::ids::AgentCommandId;
use crate::resource_access::ResourceAccessSnapshot;

use super::super::{
    AcceptedWorkPolicy, BridgeAgentTarget, BridgeCustodyPin, BridgeRecord, BridgeState,
    BridgeValidation, BridgeWorkPin,
};
use super::{BridgeInvokeError, BridgeInvokeGrant, BridgeInvokeGrantId};

/// A fixed-target grant permits only its declared use of an already retained
/// Agent. In particular no variant grants Sandbox creation, revival or approvals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeInvokeEffect {
    BindExistingTarget { allocation_id: String },
    AdmitMessage { inbox_id: String },
    ExecuteCommand { command_id: AgentCommandId },
}

impl BridgeInvokeEffect {
    pub fn validate(&self) -> Result<(), BridgeInvokeError> {
        let reference = match self {
            Self::BindExistingTarget { allocation_id } => allocation_id,
            Self::AdmitMessage { inbox_id } => inbox_id,
            Self::ExecuteCommand { command_id } => command_id.as_str(),
        };
        if !super::super::config::valid_id(reference) {
            return Err(BridgeInvokeError::Invalid("effect_reference"));
        }
        Ok(())
    }
}

pub struct BridgeInvokeRequest {
    pub grant_id: BridgeInvokeGrantId,
    pub pin: BridgeWorkPin,
    pub target: BridgeAgentTarget,
    pub effect: BridgeInvokeEffect,
}

/// Current nonrenewing effect authority. Never deserialize or persist this as a
/// future permission. The consumer must call its transaction fence immediately
/// before allocation binding, command admission or deferred dispatch.
#[derive(Clone)]
pub struct BridgeInvokeProof {
    pub(super) grant: BridgeInvokeGrant,
    pub(super) bridge: BridgeRecord,
    pub(super) resources: ResourceAccessSnapshot,
    pub(super) connection: BridgeValidation,
    pub(super) effect: BridgeInvokeEffect,
    pub(super) deadline: tokio::time::Instant,
}

impl BridgeInvokeProof {
    pub fn grant(&self) -> &BridgeInvokeGrant {
        &self.grant
    }
    pub fn bridge(&self) -> &BridgeRecord {
        &self.bridge
    }
    pub fn resources(&self) -> &ResourceAccessSnapshot {
        &self.resources
    }
    pub fn custody_pin(&self) -> &BridgeCustodyPin {
        self.connection.custody_pin()
    }
    pub fn effect(&self) -> &BridgeInvokeEffect {
        &self.effect
    }
    pub fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }

    pub fn require_current(&self) -> Result<(), BridgeInvokeError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(BridgeInvokeError::Unavailable);
        }
        self.effect.validate()?;
        self.grant.require_active()?;
        require_bridge(&self.bridge, &self.grant, &self.effect)?;
        super::service::require_resource_owner(&self.grant, &self.resources)?;
        self.connection
            .require(&self.grant.owner, &self.grant.config)?;
        Ok(())
    }

    pub fn require_snapshot(
        &self,
        bridge: &BridgeRecord,
        grant: &BridgeInvokeGrant,
        resources: &ResourceAccessSnapshot,
    ) -> Result<(), BridgeInvokeError> {
        self.require_current()?;
        if bridge != &self.bridge || grant != &self.grant {
            return Err(BridgeInvokeError::Conflict);
        }
        if resources.fence() != self.resources.fence()
            || resources.lineage != self.resources.lineage
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        super::service::require_resource_owner(&self.grant, resources)
    }
}

pub(super) fn require_bridge(
    bridge: &BridgeRecord,
    grant: &BridgeInvokeGrant,
    effect: &BridgeInvokeEffect,
) -> Result<(), BridgeInvokeError> {
    let generation = bridge.pinned(&grant.pin)?;
    if bridge.owner != grant.owner || generation.config != grant.config {
        return Err(BridgeInvokeError::TargetMismatch);
    }
    if bridge.emergency.execution_stopped {
        return Err(BridgeInvokeError::Suspended);
    }
    if let Some(channel) = &grant.channel {
        if matches!(effect, BridgeInvokeEffect::BindExistingTarget { .. }) {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        channel.require_bridge(bridge, &grant.pin, &grant.config)?;
    }
    if matches!(effect, BridgeInvokeEffect::BindExistingTarget { .. }) {
        if bridge.admission_pin()? != grant.pin {
            return Err(BridgeInvokeError::Suspended);
        }
    } else {
        // Accepted messages keep their original grant/configuration. Each newer
        // generation may explicitly suspend older work; no implicit rebinding.
        require_accepted_pin(bridge, &grant.pin)?;
    }
    Ok(())
}

pub(super) fn require_accepted_pin(
    bridge: &BridgeRecord,
    pin: &BridgeWorkPin,
) -> Result<(), BridgeInvokeError> {
    bridge.pinned(pin)?;
    if bridge.emergency.execution_stopped
        || matches!(
            bridge.state,
            BridgeState::Disabled {
                accepted_work: AcceptedWorkPolicy::SuspendOriginal
            } | BridgeState::Retired {
                accepted_work: AcceptedWorkPolicy::SuspendOriginal
            }
        )
        || bridge.generations.iter().any(|generation| {
            generation.config_revision > pin.config_revision
                && generation.previous_work == Some(AcceptedWorkPolicy::SuspendOriginal)
        })
    {
        return Err(BridgeInvokeError::Suspended);
    }
    Ok(())
}
