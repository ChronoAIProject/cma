//! Atomic first-message identity and alias receipts. Old receipts are not backfilled.
use layer2_core::bridge::ingress::{CallbackError, InboxAdmission, InboxReceipt, InboxWrite};
use mongodb::{bson::doc, ClientSession};

use super::{receipts::model::AcceptedCallbackReceipt, MongoCallbackInbox, MutationError};
mod model;
use model::{alias_id, first_id, FirstMessage, MessageAlias};

impl MongoCallbackInbox {
    pub(super) async fn resolve_platform_redelivery(
        &self,
        write: &InboxWrite,
        session: &mut ClientSession,
        create_alias: bool,
    ) -> Result<Option<InboxReceipt>, MutationError> {
        let alias = self
            .redeliveries
            .find_one(doc! {"_id":alias_id(&write.record().id)})
            .session(&mut *session)
            .await?;
        let alias = alias.as_ref().map(MessageAlias::read).transpose()?;
        let key = if let Some(alias) = alias.as_ref() {
            alias.require_exact(write)?;
            alias.key().to_owned()
        } else if let Some(proof) = write.platform_redelivery() {
            proof.key_sha256().to_owned()
        } else {
            return Ok(None);
        };
        let Some(document) = self
            .redeliveries
            .find_one(doc! {"_id":first_id(&key)})
            .session(&mut *session)
            .await?
        else {
            return if alias.is_some() {
                Err(CallbackError::Unavailable.into())
            } else {
                Ok(None)
            };
        };
        let first = FirstMessage::read(&document, &key)?;
        first.require_equivalent(write)?;
        if let Some(alias) = &alias {
            alias.require_first(&first)?;
        }
        // Reconciliation can acknowledge only an already majority-durable exact alias.
        if alias.is_none() && !create_alias {
            return Ok(None);
        }
        let document = self
            .receipts
            .find_one(doc! {"_id":first.receipt_id()})
            .session(&mut *session)
            .await?
            .ok_or(CallbackError::Unavailable)?;
        let receipt = first.require_receipt(&AcceptedCallbackReceipt::read(&document)?)?;
        if alias.is_none() {
            // Serialize the new alias with disable/rotation/retirement without
            // consuming queue/rate capacity or extending original reply custody.
            let bridge = write.bridge();
            let changed = self
                .bridges
                .definitions
                .update_one(
                    doc! {"_id":bridge.id.as_str(),"revision":bridge.revision as i64},
                    doc! {"$inc":{"inbox_fence":1_i64}},
                )
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 {
                return Err(CallbackError::BindingUnavailable.into());
            }
            self.redeliveries
                .insert_one(MessageAlias::capture(write, &first)?.document()?)
                .session(&mut *session)
                .await?;
        }
        write.require_current(write.bridge())?;
        Ok(Some(receipt))
    }

    pub(super) async fn retain_platform_identity(
        &self,
        write: &InboxWrite,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        if write.record().admission == InboxAdmission::Accepted
            && write.platform_redelivery().is_some()
        {
            self.redeliveries
                .insert_one(FirstMessage::capture(write)?.document()?)
                .session(session)
                .await?;
        }
        Ok(())
    }
}
