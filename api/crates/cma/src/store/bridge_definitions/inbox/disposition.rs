//! Shared durable Message/Control choice. Legacy acceptance wins before this path.
use layer2_core::bridge::ingress::{
    CallbackAcceptance, CallbackError, ChannelControlReceipt, InboxWrite,
};
use mongodb::{bson::doc, ClientSession};

use super::{MongoCallbackInbox, MutationError};
mod accepted;
mod model;
use model::{callback_id, provider_id, CallbackDisposition};

impl MongoCallbackInbox {
    pub(super) async fn resolve_disposition(
        &self,
        write: &InboxWrite,
        session: &mut ClientSession,
        create_alias: bool,
    ) -> Result<Option<CallbackAcceptance>, MutationError> {
        if let Some(document) = self
            .dispositions
            .find_one(doc! {"_id":callback_id(&write.record().id)})
            .session(&mut *session)
            .await?
        {
            return Ok(Some(
                CallbackDisposition::read(&document)?.require_exact(write)?,
            ));
        }
        let Some(proof) = write.platform_redelivery() else {
            return Ok(None);
        };
        let Some(document) = self
            .dispositions
            .find_one(doc! {"_id":provider_id(proof.key_sha256())})
            .session(&mut *session)
            .await?
        else {
            return Ok(None);
        };
        let original = CallbackDisposition::read_provider(&document, proof.key_sha256())?;
        let expected = original.document()?;
        let canonical = self
            .dispositions
            .find_one(doc! {"_id":expected.get_str("_id").map_err(|_|CallbackError::Unavailable)?})
            .session(&mut *session)
            .await?
            .ok_or(CallbackError::Unavailable)?;
        CallbackDisposition::read(&canonical)?;
        if canonical.get("record_sha256") != expected.get("record_sha256") {
            return Err(CallbackError::Unavailable.into());
        }
        let alias = original.alias(write)?;
        // Provider equivalence is insufficient to claim an uncertain alias committed.
        if !create_alias {
            return Ok(None);
        }
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
        let document = alias.document()?;
        self.dispositions
            .insert_one(document.clone())
            .session(&mut *session)
            .await?;
        let acceptance = alias.require_exact(write)?;
        if let CallbackAcceptance::ControlAccepted(receipt) = &acceptance {
            let database = self
                .bridges
                .client
                .database(&self.dispositions.namespace().db);
            let row = crate::store::durability::majority_collection::<mongodb::bson::Document>(
                &database,
                "bridge_channel_control_intents",
            )
            .find_one(doc! {"_id": &receipt.intent_id})
            .session(&mut *session)
            .await?
            .ok_or(CallbackError::Unavailable)?;
            let intent = crate::store::bridge_invokes::channel_controls::records::read_intent(&row)
                .map_err(|_| CallbackError::Unavailable)?;
            alias.require_retained_control(&intent)?;
            crate::store::channel_history::control_record(
                &database,
                &intent.reference,
                crate::store::channel_history::ReferenceKind::ControlDisposition,
                &document,
                session,
            )
            .await
            .map_err(|error| match error {
                crate::store::bridge_allocations::MutationError::Mongo(error) => {
                    MutationError::Mongo(error)
                }
                crate::store::bridge_allocations::MutationError::Core(_) => {
                    MutationError::Core(CallbackError::Unavailable)
                }
            })?;
        }
        Ok(Some(acceptance))
    }

    pub(super) async fn retain_disposition(
        &self,
        write: &InboxWrite,
        receipt: CallbackAcceptance,
        session: &mut ClientSession,
    ) -> Result<CallbackAcceptance, MutationError> {
        let disposition = CallbackDisposition::capture(write, receipt.clone())?;
        self.persist_disposition(&disposition, session).await?;
        Ok(receipt)
    }

    pub(in crate::store) async fn retain_accepted_control(
        &self,
        write: &layer2_core::bridge::channel_control::ChannelControlWrite,
        control: &layer2_core::bridge::channel_control::AdmittedChannelControl,
        session: &mut ClientSession,
    ) -> Result<CallbackAcceptance, MutationError> {
        let disposition = CallbackDisposition::capture_control(write, control)?;
        let receipt = disposition.require_exact(write.inbox())?;
        self.persist_disposition(&disposition, session).await?;
        Ok(receipt)
    }

    async fn persist_disposition(
        &self,
        disposition: &CallbackDisposition,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        self.dispositions
            .insert_one(disposition.document()?)
            .session(&mut *session)
            .await?;
        if let Some(provider) = disposition.provider_document()? {
            self.dispositions
                .insert_one(provider)
                .session(&mut *session)
                .await?;
        }
        Ok(())
    }
}

pub(super) fn refusal(write: &InboxWrite) -> Option<CallbackAcceptance> {
    write.control_refusal().map(|code| {
        CallbackAcceptance::ControlRefused(ChannelControlReceipt {
            version: 1,
            callback_id: write.record().id.clone(),
            bridge_id: write.record().pin.bridge_id.clone(),
            message_id: write.record().message_id.clone(),
            persisted_at: write.record().accepted_at,
            code,
        })
    })
}
