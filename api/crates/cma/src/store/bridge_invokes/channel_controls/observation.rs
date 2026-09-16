//! Trusted current observations for a freshly authenticated control callback.
//! A snapshot is data; the separate invoke service supplies live authority.
use super::*;
use layer2_core::bridge::{
    invoke::{BridgeInvokeEffect, BridgeInvokeRequest, BridgeInvokeService},
    outbox::{ChannelControlReferenceRecord, ChannelControlThread, OutboxItem},
};
use layer2_core::sandbox::agent_io::{AdmittedAgentCommand, ChannelCommandExecutionSource};

struct Snapshot {
    reference: ChannelControlReferenceRecord,
    command: AdmittedAgentCommand,
    source: ChannelCommandExecutionSource,
    consent: layer2_core::bridge::invoke::SourceControlPolicy,
    consent_receipt: layer2_core::bridge::invoke::SourceControlReceipt,
    replied: Option<OutboxItem>,
}

impl MongoChannelControlStore {
    pub(crate) async fn admission_proof(
        &self,
        write: ChannelControlWrite,
        invokes: &BridgeInvokeService,
    ) -> Result<ChannelControlProof, Error> {
        let deadline = write.inbox().deadline();
        tokio::time::timeout_at(deadline, async {
            write.require_current()?;
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| Error::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| Error::Unavailable)?;
            let snapshot = self.observe(&write, &mut session).await;
            // reference_consent uses a policy fence. This observation transaction
            // is deliberately aborted: the admission transaction repeats and
            // commits every canonical source, consent and reply fence together.
            let _ = session.abort_transaction().await;
            let snapshot = snapshot.map_err(MutationError::core)?;
            write.require_current()?;
            let binding = snapshot.reference.binding();
            let proof = invokes
                .authorize(BridgeInvokeRequest {
                    grant_id: binding.consent.view.grant_id.clone(),
                    pin: binding.source.pin().clone(),
                    target: binding.source.target().clone(),
                    effect: BridgeInvokeEffect::ExecuteCommand {
                        command_id: binding.source.command_id().clone(),
                    },
                })
                .await
                .map_err(|_| Error::AuthorityUnavailable)?;
            ChannelControlProof::from_current(
                write,
                proof,
                ChannelControlObservation {
                    reference: &snapshot.reference,
                    original_inbox: &snapshot.source.inbox,
                    source_command: &snapshot.command,
                    source_sidecar: &snapshot.source.sidecar,
                    allocation: &snapshot.source.allocation,
                    consent: &snapshot.consent,
                    consent_receipt: &snapshot.consent_receipt,
                    replied_item: snapshot.replied.as_ref(),
                },
            )
        })
        .await
        .map_err(|_| Error::Unavailable)?
    }

    async fn observe(
        &self,
        write: &ChannelControlWrite,
        session: &mut ClientSession,
    ) -> Result<Snapshot, MutationError> {
        write.require_current()?;
        let database = self
            .invokes
            .bridges
            .client
            .database(&self.intents.namespace().db);
        let row = database
            .collection::<Document>("bridge_channel_control_references")
            .find_one(doc! {"_id":&write.request().reference})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceUnavailable)?;
        let reference =
            super::super::outbox::read_control_reference(&row).map_err(|_| Error::Corrupt)?;
        let binding = reference.binding();
        let signed = write.source();
        // Reject a copied reference before reading another source's grant or
        // making any external authority observation. The core proof repeats
        // these checks against the fresh source authority and full snapshot.
        if reference.reference != write.request().reference
            || write.inbox().record().pin != *binding.source.pin()
            || write.inbox().record().owner != *binding.source.owner()
            || signed.bot_id() != binding.source.config().connector.bot_id
            || signed.issuer() != binding.source.config().connector.issuer
            || signed.platform() != &binding.source.config().connector.platform
            || signed.chat_id() != binding.source.chat_id()
            || signed.sender_id() != binding.sender_id
            || ChannelControlThread::capture(signed.thread()) != binding.thread
        {
            return Err(Error::SourceUnavailable.into());
        }
        let row = self
            .invokes
            .resources
            .agent_commands
            .find_one(doc! {"_id":binding.source.command_id().as_str()})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceUnavailable)?;
        let command = read_command(&row).map_err(|_| Error::Corrupt)?;
        let source = self.invokes.read_channel_source(&command, session).await?;
        let row = self
            .invokes
            .grants
            .find_one(doc! {"_id":binding.consent.view.grant_id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceUnavailable)?;
        let grant = super::super::read_grant(&row).map_err(|_| Error::Corrupt)?;
        let (consent, consent_receipt) = self
            .invokes
            .reference_consent(&grant, write.inbox().deadline(), session)
            .await?
            .ok_or(Error::AuthorityUnavailable)?;
        let replied = if let Some(evidence) = write
            .inbox()
            .record()
            .capabilities
            .as_ref()
            .and_then(|facts| facts.confirmed_reply_receipt())
        {
            let row = database
                .collection::<Document>("bridge_outbox_items")
                .find_one(doc! {"_id":&evidence.item_id})
                .session(&mut *session)
                .await?
                .ok_or(Error::SourceUnavailable)?;
            crate::store::MongoBridgeReplyToolStore::verify_confirmed_reply(
                &row,
                write.inbox().record(),
                evidence,
            )
            .map_err(|_| Error::SourceUnavailable)?;
            Some(super::super::outbox::read_cleanup_item(&row).map_err(|_| Error::Corrupt)?)
        } else {
            None
        };
        write.require_current()?;
        Ok(Snapshot {
            reference,
            command,
            source,
            consent,
            consent_receipt,
            replied,
        })
    }
}
