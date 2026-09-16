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
    /// Structural preparation only. A detach followed by reset advances the
    /// old row twice; the effect store must prove both permanent command journals.
    /// This method does not establish the existence of either journal.
    pub fn require_retained_execution_shape(
        &self,
        current: &AllocationRecord,
    ) -> Result<(), BridgeInvokeError> {
        if self.require_retained_allocation(current).is_ok() {
            return Ok(());
        }
        current.validate().map_err(|_| BridgeInvokeError::Corrupt)?;
        let mut retired = self.allocation.clone();
        if retired.state != AllocationState::Ready
            || current.state != AllocationState::Retired
            || current.updated_at_ms < retired.updated_at_ms
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        retired.state = AllocationState::Retired;
        retired.lease = None;
        retired.revision = retired
            .revision
            .checked_add(2)
            .ok_or(BridgeInvokeError::Corrupt)?;
        retired.updated_at_ms = current.updated_at_ms;
        if retired != *current {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        Ok(())
    }
    pub fn validate(&self, grant: &BridgeInvokeGrant) -> Result<(), BridgeInvokeError> {
        let allocation = &self.allocation;
        allocation
            .validate()
            .map_err(|_| BridgeInvokeError::Corrupt)?;
        let profiles = allocation
            .placement
            .profiles()
            .ok_or(BridgeInvokeError::TargetMismatch)?;
        let target = allocation.placement.target();
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

    /// Existing commands retain their original Ready creation evidence. Reset
    /// changes only this exact terminal state, revision and timestamp; storage
    /// must additionally prove/fence the canonical successor and current head.
    /// This comparison never authorizes a new grant or command admission.
    pub fn require_retained_allocation(
        &self,
        current: &AllocationRecord,
    ) -> Result<(), BridgeInvokeError> {
        current.validate().map_err(|_| BridgeInvokeError::Corrupt)?;
        if current == &self.allocation {
            return Ok(());
        }
        let mut retired = self.allocation.clone();
        if retired.state != AllocationState::Ready
            || current.state != AllocationState::Retired
            || current.updated_at_ms < retired.updated_at_ms
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        retired.state = AllocationState::Retired;
        retired.lease = None;
        retired.revision = retired
            .revision
            .checked_add(1)
            .ok_or(BridgeInvokeError::Corrupt)?;
        retired.updated_at_ms = current.updated_at_ms;
        if &retired != current {
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
        let profiles = self
            .allocation
            .placement
            .profiles()
            .ok_or(BridgeInvokeError::TargetMismatch)?;
        let target = self.allocation.placement.target();
        if let AllocationPlacement::ReuseParent { parent, .. } = &self.allocation.placement {
            parent
                .require(&self.allocation.config, profiles, resources)
                .map_err(|_| BridgeInvokeError::TargetMismatch)?;
        }
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
    right.placement.creates_partition_agent()
        && left.connector.issuer == right.connector.issuer
        && left.connector.bot_id == right.connector.bot_id
        && left.connector.platform == right.connector.platform
        && left.partition == right.partition
}
