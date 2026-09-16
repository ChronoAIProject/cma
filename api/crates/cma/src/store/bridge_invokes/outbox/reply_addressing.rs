//! Indexed read of permanent Sent evidence; no delivery or current effect grant.
use super::*;
use crate::bridge::ingress::reply_addressing::{ConfirmedReply, ReplyAddressingStore, ReplyLookup};
use futures_util::TryStreamExt;
use layer2_core::bridge::ingress::CallbackError;

#[async_trait::async_trait]
impl ReplyAddressingStore for MongoBridgeReplyToolStore {
    async fn confirmed_reply(
        &self,
        lookup: &ReplyLookup,
    ) -> Result<Option<ConfirmedReply>, CallbackError> {
        let platform = mongodb::bson::serialize_to_bson(&lookup.platform)
            .map_err(|_| CallbackError::Unavailable)?;
        let rows = self.items.find(doc! {
            "state":"sent", "record.source.pin.bridge_id":lookup.source.work_pin().bridge_id.as_str(),
            "record.source.config.connector.issuer":&lookup.issuer,
            "record.source.config.connector.bot_id":&lookup.bot_id,
            "record.source.config.connector.platform":platform,
            "record.source.chat_id":&lookup.chat_id,
            "record.state.receipt.platform_message_id":&lookup.parent_id,
        }).limit(16).await.map_err(|_| CallbackError::Unavailable)?
            .try_collect::<Vec<Document>>().await.map_err(|_| CallbackError::Unavailable)?;
        let mut confirmed = None;
        for row in rows {
            // A matching but inconsistent row is an unavailable observation;
            // never convert storage uncertainty into permanent sender denial.
            let item = records::read_item(&row).map_err(|_| CallbackError::Unavailable)?;
            if let Some(proof) = lookup.confirm(&item)? {
                confirmed = Some(proof);
            }
        }
        Ok(confirmed)
    }
}

impl MongoBridgeReplyToolStore {
    /// Used by callback storage in the same transaction as fresh acceptance.
    pub(crate) fn verify_confirmed_reply(
        document: &Document,
        inbox: &layer2_core::bridge::ingress::StoredInbox,
        expected: &layer2_core::bridge::partition::ConfirmedReplyReceipt,
    ) -> Result<(), CallbackError> {
        expected
            .validate()
            .map_err(|_| CallbackError::Unavailable)?;
        let item = records::read_item(document).map_err(|_| CallbackError::Unavailable)?;
        let proof = ReplyLookup::retained(inbox)?
            .confirm(&item)?
            .ok_or(CallbackError::Unavailable)?;
        if proof.receipt() != expected {
            return Err(CallbackError::Unavailable);
        }
        Ok(())
    }
}
