//! Invocation authority for one genuinely channel-created resource tree.
use serde::{Deserialize, Serialize};

use crate::bridge::allocation::{AllocationPlacement, AllocationRecord, AllocationState};
use crate::resource_access::ResourceAccessSnapshot;
use crate::sandbox::{AgentCreationOrigin, AgentCreationProvenance};

use super::{BridgeInvokeError, BridgeInvokeGrant, BridgeSharedContextConsent};

/// Immutable creation evidence retained with the grant. Current store reads and
/// transaction fences must still prove this exact allocation and provenance.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelInvokeBasis {
    pub allocation: AllocationRecord,
    pub creation: AgentCreationProvenance,
}

impl std::fmt::Debug for ChannelInvokeBasis {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChannelInvokeBasis")
            .finish_non_exhaustive()
    }
}

pub struct ChannelInvokeSnapshot {
    pub basis: ChannelInvokeBasis,
    pub resources: ResourceAccessSnapshot,
}

impl ChannelInvokeBasis {
    pub fn validate(&self, grant: &BridgeInvokeGrant) -> Result<(), BridgeInvokeError> {
        let allocation = &self.allocation;
        allocation
            .validate()
            .map_err(|_| BridgeInvokeError::Corrupt)?;
        let AllocationPlacement::NewSandbox { profiles, target } = &allocation.placement else {
            return Err(BridgeInvokeError::TargetMismatch);
        };
        let creation = &self.creation;
        if allocation.state != AllocationState::Ready
            || allocation.lease.is_some()
            || allocation.owner != grant.owner
            || allocation.pin.bridge_id != grant.pin.bridge_id
            || !same_partition_namespace(&allocation.config, &grant.config)
            || target != &grant.target.ids
            || grant.consent != BridgeSharedContextConsent::UseChannelCreatedAgent
            || creation.owner != allocation.owner
            || creation.agent_id != target.agent_id
            || creation.workspace_id != target.workspace_id
            || creation.worktree_id != target.worktree_id
            || creation.sandbox_id != target.sandbox_id
            || creation.agent_created_at != grant.target.agent_created_at
            || creation.sandbox_created_at != grant.target.sandbox_created_at
            || creation.created_by.as_ref() != Some(&profiles.approved_by)
            || creation.creation_fingerprint != grant.target.agent_creation_fingerprint
            || creation.creation_fingerprint
                != crate::sandbox::channel_fingerprint(allocation)
                    .map_err(|_| BridgeInvokeError::Corrupt)?
            || creation.creation_origin
                != (AgentCreationOrigin::ChannelBridge {
                    bridge_id: allocation.identity.bridge_id.as_str().into(),
                    partition_id: allocation.identity.key.hash().into(),
                    generation: allocation.identity.generation,
                    source_reference: allocation.first_source.inbox_id().into(),
                })
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        Ok(())
    }

    /// Inbound policy and creation policy are independent immutable generations.
    /// Both must remain admitted under this same current Bridge snapshot.
    pub fn require_bridge(
        &self,
        bridge: &crate::bridge::BridgeRecord,
        inbound: &crate::bridge::BridgeWorkPin,
        config: &crate::bridge::BridgeConfig,
    ) -> Result<(), BridgeInvokeError> {
        if bridge.owner != self.allocation.owner
            || inbound.bridge_id != self.allocation.pin.bridge_id
            || bridge.pinned(inbound)?.config != *config
            || bridge.pinned(&self.allocation.pin)?.config != self.allocation.config
            || !same_partition_namespace(&self.allocation.config, config)
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        super::proof::require_accepted_pin(bridge, inbound)?;
        super::proof::require_accepted_pin(bridge, &self.allocation.pin)
    }

    pub fn require_resources(
        &self,
        resources: &ResourceAccessSnapshot,
    ) -> Result<(), BridgeInvokeError> {
        let AllocationPlacement::NewSandbox { profiles, target } = &self.allocation.placement
        else {
            return Err(BridgeInvokeError::TargetMismatch);
        };
        let agent = resources
            .lineage
            .agent
            .as_ref()
            .ok_or(BridgeInvokeError::TargetMismatch)?;
        let workspace = resources
            .lineage
            .workspace
            .as_ref()
            .ok_or(BridgeInvokeError::TargetMismatch)?;
        self.creation
            .validate_against(agent, &resources.lineage.sandbox)
            .map_err(|_| BridgeInvokeError::TargetMismatch)?;
        if agent.id != target.agent_id
            || resources.lineage.sandbox.profile != profiles.sandbox
            || workspace.profile != profiles.workspace
            || agent.profile != profiles.agent
            || agent.creation_key != format!("channel-allocation:{}", self.allocation.id)
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        Ok(())
    }
}

fn same_partition_namespace(
    left: &crate::bridge::BridgeConfig,
    right: &crate::bridge::BridgeConfig,
) -> bool {
    matches!(
        right.placement,
        crate::bridge::BridgePlacementPolicy::NewSandbox { .. }
    ) && left.connector.issuer == right.connector.issuer
        && left.connector.bot_id == right.connector.bot_id
        && left.connector.platform == right.connector.platform
        && left.partition == right.partition
}
