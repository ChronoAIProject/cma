//! Positive reply admission and its immutable confirmed receipt share one write.
use super::*;

impl MongoCallbackInbox {
    pub(super) async fn require_confirmed_reply(
        &self,
        inbox: &StoredInbox,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        if inbox.admission != InboxAdmission::Accepted {
            return Ok(());
        }
        let Some(receipt) = inbox
            .capabilities
            .as_ref()
            .and_then(|facts| facts.confirmed_reply_receipt())
        else {
            return Ok(());
        };
        let items = majority_collection::<Document>(
            &self.bridges.client.database(&self.inbox.namespace().db),
            "bridge_outbox_items",
        );
        let document = items
            .find_one(doc! {"_id":&receipt.item_id})
            .session(&mut *session)
            .await?
            .ok_or(CallbackError::Unavailable)?;
        crate::store::MongoBridgeReplyToolStore::verify_confirmed_reply(&document, inbox, receipt)?;
        let changed = items.update_one(
            doc! {"_id":&receipt.item_id,"revision":receipt.revision as i64,"record":document.get("record").cloned().ok_or(CallbackError::Unavailable)?},
            doc! {"$inc":{"reply_addressing_fence":1_i64}},
        ).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(CallbackError::Unavailable.into());
        }
        Ok(())
    }
}
