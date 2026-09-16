use super::*;
use layer2_core::bridge::outbox::ChannelControlReferenceRecord;

impl MongoChannelControlStore {
    pub(super) async fn require_unclaimed_callback(
        &self,
        write: &ChannelControlWrite,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let callback = ChannelControlCallback::capture(write)?;
        let database = self
            .invokes
            .bridges
            .client
            .database(&self.intents.namespace().db);
        for (name, query) in [
            (
                "bridge_callback_dispositions",
                doc! {"_id":{"$in":[format!("callback:{}",callback.callback_id),format!("provider:{}",callback.provider_key_sha256)]}},
            ),
            (
                "bridge_callback_receipts",
                doc! {"_id":&callback.callback_id},
            ),
            ("bridge_callback_inbox", doc! {"_id":&callback.callback_id}),
            (
                "bridge_callback_platform_messages",
                doc! {"_id":format!("message:{}",callback.provider_key_sha256)},
            ),
            (
                "bridge_refused_control_intents",
                doc! {"_id":callback.intent_id()?},
            ),
        ] {
            if database
                .collection::<Document>(name)
                .find_one(query)
                .session(&mut *session)
                .await?
                .is_some()
            {
                return Err(Error::Conflict.into());
            }
        }
        Ok(())
    }

    pub(super) async fn fence_source(
        &self,
        proof: &ChannelControlProof,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        self.invokes
            .fence_transaction(proof.source_invoke(), session)
            .await?;
        let (consent, receipt) = self
            .invokes
            .reference_consent(
                proof.source_invoke().grant(),
                proof.source_invoke().deadline(),
                session,
            )
            .await?
            .ok_or(Error::AuthorityUnavailable)?;
        let binding = proof.intent().reference.binding();
        if consent != binding.consent || receipt != binding.consent_receipt {
            return Err(Error::AuthorityUnavailable.into());
        }
        let database = self
            .invokes
            .bridges
            .client
            .database(&self.intents.namespace().db);
        let references = database.collection::<Document>("bridge_channel_control_references");
        let reference = references
            .find_one(doc! {"_id":&proof.intent().reference.reference})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceUnavailable)?;
        let current: ChannelControlReferenceRecord =
            crate::store::sandbox_mongo::read_record(&reference).map_err(|_| Error::Corrupt)?;
        current.validate().map_err(|_| Error::Corrupt)?;
        if current != proof.intent().reference
            || reference.get_str("lookup_sha256").ok() != Some(current.lookup_sha256.as_str())
        {
            return Err(Error::SourceUnavailable.into());
        }
        let changed = references.update_one(doc! {"_id":&current.reference,"record":record_bson(&current).map_err(|_| Error::Corrupt)?},
            doc! {"$inc":{"control_admission_fence":1_i64}}).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(Error::Conflict.into());
        }
        super::super::channel_source::fence(
            &self.invokes,
            proof.source_command(),
            proof.source_sidecar(),
            proof.original_inbox(),
            session,
        )
        .await?;
        let source_document = self
            .invokes
            .resources
            .agent_commands
            .find_one(doc! {"_id":proof.source_command().receipt.envelope.command_id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceUnavailable)?;
        if source_document.get("channel_admission")
            != Some(&record_bson(proof.source_sidecar()).map_err(|_| Error::Corrupt)?)
        {
            return Err(Error::SourceUnavailable.into());
        }
        let allocations = database.collection::<Document>("bridge_allocations");
        let allocation = allocations
            .find_one(doc! {"_id":&proof.allocation().id})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceUnavailable)?;
        if crate::store::bridge_allocations::decode_record(&allocation)
            .map_err(|_| Error::Corrupt)?
            != *proof.allocation()
        {
            return Err(Error::SourceUnavailable.into());
        }
        let changed = allocations
            .update_one(
                doc! {"_id":&proof.allocation().id,"revision":proof.allocation().revision as i64},
                doc! {"$inc":{"control_admission_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(Error::Conflict.into());
        }
        if let Some(item) = proof.replied_item() {
            let items = database.collection::<Document>("bridge_outbox_items");
            let document = items
                .find_one(doc! {"_id":&item.id})
                .session(&mut *session)
                .await?
                .ok_or(Error::SourceUnavailable)?;
            let evidence = proof
                .write()
                .inbox()
                .record()
                .capabilities
                .as_ref()
                .and_then(|facts| facts.confirmed_reply_receipt())
                .ok_or(Error::SourceUnavailable)?;
            crate::store::MongoBridgeReplyToolStore::verify_confirmed_reply(
                &document,
                proof.write().inbox().record(),
                evidence,
            )
            .map_err(|_| Error::SourceUnavailable)?;
            let changed = items
                .update_one(
                    doc! {"_id":&item.id,"record":record_bson(item).map_err(|_| Error::Corrupt)?},
                    doc! {"$inc":{"control_admission_fence":1_i64}},
                )
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 {
                return Err(Error::Conflict.into());
            }
        }
        proof.require_current()?;
        Ok(())
    }
}
