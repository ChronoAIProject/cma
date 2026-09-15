mod types;
mod validation;
use super::{Bridges, validation as bridge_validation};
use crate::{Error, IdempotencyKey, PageOptions, Result};
use reqwest::{Method, StatusCode};
pub use types::*;

impl Bridges<'_> {
    /// List current owner/admin delivery history for one exact original Bridge.
    pub async fn list_deliveries(
        &self,
        bridge_id: &str,
        options: &PageOptions,
    ) -> Result<BridgeDeliveryPage> {
        bridge_validation::id(bridge_id)?;
        if options.limit.is_some_and(|limit| limit == 0 || limit > 100) {
            return Err(Error::validation("delivery limit must be 1 through 100"));
        }
        if let Some(cursor) = &options.cursor {
            validation::item_id(cursor)?;
        }
        let page: BridgeDeliveryPage = self
            .client
            .get(&["bridges", bridge_id, "deliveries"], &options.query())
            .await?;
        if page.items.len() > usize::from(options.limit.unwrap_or(50)) {
            return Err(Error::protocol(
                "delivery page exceeded its requested limit",
            ));
        }
        let mut previous = options.cursor.as_deref();
        for item in &page.items {
            item.check(bridge_id)?;
            if previous.is_some_and(|id| id >= item.id.as_str()) {
                return Err(Error::protocol("delivery page did not advance"));
            }
            previous = Some(&item.id);
        }
        if page
            .next_cursor
            .as_deref()
            .is_some_and(|cursor| Some(cursor) != previous || page.items.is_empty())
        {
            return Err(Error::protocol(
                "delivery continuation does not match its page",
            ));
        }
        Ok(page)
    }
    /// Inspect safe status and attempts; text, destination and secret references are excluded.
    pub async fn read_delivery(&self, bridge_id: &str, item_id: &str) -> Result<BridgeDelivery> {
        bridge_validation::id(bridge_id)?;
        validation::item_id(item_id)?;
        let item: BridgeDelivery = self
            .client
            .get(&["bridges", bridge_id, "deliveries", item_id], &[])
            .await?;
        item.check(bridge_id)?;
        if item.id != item_id {
            return Err(Error::protocol("delivery identity changed"));
        }
        Ok(item)
    }
    /// Explicit recovery with durable idempotency. After cancellation or response
    /// loss retry the unchanged request and key; this never retargets the item.
    pub async fn recover_delivery(
        &self,
        bridge_id: &str,
        body: &BridgeDeliveryRecoveryRequest,
        key: &IdempotencyKey,
    ) -> Result<BridgeDeliveryRecoveryReceipt> {
        bridge_validation::id(bridge_id)?;
        validation::item_id(&body.item_id)?;
        bridge_validation::revision(body.expected_revision)?;
        bridge_validation::revision(body.expected_bridge_revision)?;
        let receipt: BridgeDeliveryRecoveryReceipt = self
            .client
            .command(
                Method::POST,
                &["bridges", bridge_id, "delivery-recovery"],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await?;
        receipt.check(bridge_id, body)?;
        Ok(receipt)
    }
}
