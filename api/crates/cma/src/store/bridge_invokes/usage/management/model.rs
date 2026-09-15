use layer2_core::bridge::{BridgeWrite, StoredBridgeReceipt};
use serde::{Deserialize, Serialize};

use super::*;

/// The first management transition that closed this allocation to new bindings.
/// Original DrainOriginal work may use it historically under its own authority.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store::bridge_invokes::usage) struct UsageClosure {
    schema_version: u16,
    allocation_id: String,
    pub invalid_through_config_revision: u64,
    pub management: StoredBridgeReceipt,
    pub binding: Option<ChannelBindingReference>,
}

impl UsageClosure {
    pub fn new(
        allocation: &AllocationRecord,
        previous: &BridgeRecord,
        write: &BridgeWrite,
        head: Option<&BindingHead>,
    ) -> Result<Self, BridgeInvokeError> {
        let result = Self {
            schema_version: 1,
            allocation_id: allocation.id.clone(),
            invalid_through_config_revision: previous.config_revision,
            management: write.receipt().clone(),
            binding: head.map(|head| head.binding.clone()),
        };
        result.require(allocation)?;
        Ok(result)
    }

    pub fn read(
        document: &Document,
        allocation: &AllocationRecord,
    ) -> Result<Option<Self>, BridgeInvokeError> {
        document
            .get("channel_usage_closure")
            .map(|value| {
                let closure: Self = mongodb::bson::deserialize_from_bson(value.clone())
                    .map_err(|_| BridgeInvokeError::Corrupt)?;
                closure.require(allocation)?;
                Ok(closure)
            })
            .transpose()
    }

    fn require(&self, allocation: &AllocationRecord) -> Result<(), BridgeInvokeError> {
        let receipt = &self.management.receipt;
        if self.schema_version != 1
            || self.allocation_id != allocation.id
            || receipt.bridge_id != allocation.identity.bridge_id
            || receipt.owner != allocation.owner
            || receipt.revision < 2
            || receipt.revision > i64::MAX as u64
            || receipt.config_revision == 0
            || receipt.config_revision > receipt.revision
            || self.invalid_through_config_revision < allocation.pin.config_revision
            || !(receipt.command == "update"
                && self.invalid_through_config_revision.checked_add(1)
                    == Some(receipt.config_revision)
                || receipt.command == "retire"
                    && receipt.config_revision == self.invalid_through_config_revision)
            || layer2_core::seconds_from_rfc3339(&receipt.applied_at).is_none()
            || [&self.management.id, &self.management.command_digest]
                .iter()
                .any(|id| id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(BridgeInvokeError::Corrupt);
        }
        if let Some(binding) = &self.binding {
            binding.validate().map_err(|_| BridgeInvokeError::Corrupt)?;
            if binding.bridge_id != allocation.identity.bridge_id.as_str()
                || binding.partition_id != allocation.identity.key.hash()
            {
                return Err(BridgeInvokeError::TargetMismatch);
            }
        }
        Ok(())
    }
}
