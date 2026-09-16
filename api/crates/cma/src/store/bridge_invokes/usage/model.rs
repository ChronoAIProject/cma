use serde::{Deserialize, Serialize};

use super::*;

/// One real allocation may have several live configuration-grant links. The
/// usage generation counts association incarnations, separately from either
/// Bridge configuration or allocation reset generation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingHead {
    pub schema_version: u16,
    pub revision: u64,
    pub allocation_id: String,
    pub creation: AgentCreationProvenance,
    pub binding: ChannelBindingReference,
    pub linked_grants: u64,
}

impl BindingHead {
    pub fn read(document: &Document) -> Result<Option<Self>, BridgeInvokeError> {
        document
            .get("channel_usage_binding")
            .map(|value| {
                let head: Self = mongodb::bson::deserialize_from_bson(value.clone())
                    .map_err(|_| BridgeInvokeError::Corrupt)?;
                head.validate()?;
                Ok(head)
            })
            .transpose()
    }

    fn validate(&self) -> Result<(), BridgeInvokeError> {
        self.binding
            .validate()
            .map_err(|_| BridgeInvokeError::Corrupt)?;
        if self.schema_version != 1
            || self.revision == 0
            || self.revision >= i64::MAX as u64
            || self.linked_grants >= i64::MAX as u64
            || !self.allocation_id.starts_with("bral_")
            || !self.binding.binding_id.starts_with("bruse_")
        {
            return Err(BridgeInvokeError::Corrupt);
        }
        Ok(())
    }

    pub fn require_allocation(
        &self,
        allocation: &AllocationRecord,
    ) -> Result<(), BridgeInvokeError> {
        self.validate()?;
        if self.allocation_id != allocation.id
            || self.binding.bridge_id != allocation.identity.bridge_id.as_str()
            || self.binding.partition_id != allocation.identity.key.hash()
            || self.creation.agent_id != allocation.placement.target().agent_id
            || self.creation.owner != allocation.owner
            || self.creation.sandbox_id != allocation.placement.target().sandbox_id
            || self.creation.workspace_id != allocation.placement.target().workspace_id
            || self.creation.worktree_id != allocation.placement.target().worktree_id
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GrantLink {
    pub schema_version: u16,
    pub grant_id: BridgeInvokeGrantId,
    pub allocation_id: String,
    pub binding: ChannelBindingReference,
    pub agent_id: layer2_core::AgentId,
    pub agent_created_at: String,
    pub active: bool,
}

impl GrantLink {
    pub fn new(grant: &BridgeInvokeGrant, head: &BindingHead) -> Self {
        Self {
            schema_version: 1,
            grant_id: grant.id.clone(),
            allocation_id: head.allocation_id.clone(),
            binding: head.binding.clone(),
            agent_id: head.creation.agent_id.clone(),
            agent_created_at: head.creation.agent_created_at.clone(),
            active: true,
        }
    }

    pub fn read(
        document: &Document,
        grant: &BridgeInvokeGrant,
    ) -> Result<Option<Self>, BridgeInvokeError> {
        document
            .get("channel_usage_binding")
            .map(|value| {
                let link: Self = mongodb::bson::deserialize_from_bson(value.clone())
                    .map_err(|_| BridgeInvokeError::Corrupt)?;
                link.binding
                    .validate()
                    .map_err(|_| BridgeInvokeError::Corrupt)?;
                if link.schema_version != 1
                    || link.grant_id != grant.id
                    || link.binding.bridge_id != grant.pin.bridge_id.as_str()
                    || link.agent_id != grant.target.ids.agent_id
                    || link.agent_created_at != grant.target.agent_created_at
                    || !link.allocation_id.starts_with("bral_")
                {
                    return Err(BridgeInvokeError::TargetMismatch);
                }
                Ok(link)
            })
            .transpose()
    }

    pub fn require_head(&self, head: &BindingHead) -> Result<(), BridgeInvokeError> {
        if self.allocation_id != head.allocation_id
            || self.binding != head.binding
            || self.agent_id != head.creation.agent_id
            || self.agent_created_at != head.creation.agent_created_at
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        Ok(())
    }
}
