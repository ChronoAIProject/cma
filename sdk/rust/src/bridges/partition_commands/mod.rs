mod types;
mod validation;
use super::{Bridges, validation as bridge};
use crate::{Error, IdempotencyKey, Result};
use reqwest::{Method, StatusCode};
pub use types::*;

impl Bridges<'_> {
    /// Reserve a new binding generation while retaining the original configuration,
    /// accepted work and resources. Existing-Agent native context remains unchanged.
    /// Preserve the exact path, body and key after cancellation or response loss.
    pub async fn reset_partition(
        &self,
        bridge_id: &str,
        allocation_id: &str,
        body: &BridgePartitionCommandRequest,
        key: &IdempotencyKey,
    ) -> Result<BridgePartitionCommandReceipt> {
        self.partition_command(
            bridge_id,
            allocation_id,
            body,
            key,
            BridgePartitionCommandAction::Reset,
        )
        .await
    }
    /// Stop future input for this binding. Existing work and history remain retained;
    /// resuming requires an explicit reset. This does not request resource cleanup.
    pub async fn detach_partition(
        &self,
        bridge_id: &str,
        allocation_id: &str,
        body: &BridgePartitionCommandRequest,
        key: &IdempotencyKey,
    ) -> Result<BridgePartitionCommandReceipt> {
        self.partition_command(
            bridge_id,
            allocation_id,
            body,
            key,
            BridgePartitionCommandAction::Detach,
        )
        .await
    }
    async fn partition_command(
        &self,
        bridge_id: &str,
        allocation_id: &str,
        body: &BridgePartitionCommandRequest,
        key: &IdempotencyKey,
        action: BridgePartitionCommandAction,
    ) -> Result<BridgePartitionCommandReceipt> {
        bridge::id(bridge_id)?;
        if !validation::allocation_id(allocation_id) {
            return Err(Error::validation("invalid partition identity"));
        }
        bridge::revision(body.expected_bridge_revision)?;
        bridge::revision(body.expected_revision)?;
        if body.expected_revision >= i64::MAX as u64 {
            return Err(Error::validation("partition revision cannot advance"));
        }
        let operation = match action {
            BridgePartitionCommandAction::Reset => "reset",
            BridgePartitionCommandAction::Detach => "detach",
        };
        let receipt: BridgePartitionCommandReceipt = self
            .client
            .command(
                Method::POST,
                &["bridges", bridge_id, "partitions", allocation_id, operation],
                Some(body),
                key,
                StatusCode::ACCEPTED,
            )
            .await?;
        receipt.check(bridge_id, allocation_id, body, action)?;
        Ok(receipt)
    }
}
