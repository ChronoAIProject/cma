use super::*;
use crate::{
    bridge::{BridgeError, BridgeId, BridgeService},
    resource_access::CallerContext,
};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct BridgePartitionCommandService {
    definitions: BridgeService,
    store: Arc<dyn BridgePartitionCommandStore>,
}
impl BridgePartitionCommandService {
    pub fn new(definitions: BridgeService, store: Arc<dyn BridgePartitionCommandStore>) -> Self {
        Self { definitions, store }
    }
    pub async fn command(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge_id: &BridgeId,
        key: &str,
        request: PartitionCommandRequest,
    ) -> Result<PartitionCommandReceipt, PartitionCommandError> {
        if key.is_empty()
            || key.len() > 256
            || !key.bytes().all(|c| c.is_ascii_graphic())
            || !allocation_id(&request.allocation_id)
            || request.expected_revision == 0
            || request.expected_bridge_revision == 0
            || request.expected_revision >= i64::MAX as u64
            || request.expected_bridge_revision > i64::MAX as u64
        {
            return Err(PartitionCommandError::Invalid);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let bridge = self
            .definitions
            .store
            .bridge(bridge_id)
            .await?
            .ok_or(BridgeError::NotFound)?;
        let mut manager = self
            .definitions
            .management_lease(context, credential, bridge_id, bridge.revision)
            .await?;
        manager.bound_deadline(deadline);
        manager.require_current()?;
        let receipt_id = format!(
            "bpc_{}",
            command_hash(&(
                "cma.bridge.partition-command.v1",
                &context.credential.issuer,
                manager.actor(),
                bridge_id,
                key
            ))?
        );
        let request_sha256 = command_hash(&request)?;
        // Current management authority always precedes recovery. Exact original
        // receipts precede stale revisions and any live provisioning preflight.
        if let Some(receipt) = self
            .store
            .recover(&manager, &receipt_id, &request_sha256)
            .await?
        {
            receipt.validate()?;
            self.definitions.validate_management(&manager).await?;
            return Ok(receipt);
        }
        if manager.record().revision != request.expected_bridge_revision {
            return Err(PartitionCommandError::Conflict);
        }
        let snapshot = self
            .store
            .snapshot(&manager, &request.allocation_id)
            .await?;
        let write = PartitionCommandWrite {
            manager,
            request,
            receipt_id,
            request_sha256,
            snapshot,
        };
        write.require_current()?;
        self.store.apply(write).await
    }
}
