//! Private URL custody. No public download endpoint or ambient object reader.
use layer2_core::bridge::{
    ingress::{CallbackError, InboxAdmission, InboxWrite, StoredInbox},
    invoke::{BridgeInvokeEffect, BridgeInvokeProof},
};
use mongodb::{
    bson::{doc, DateTime},
    ClientSession,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{MongoCallbackInbox, MutationError};
use crate::store::{encryption::CipherEnvelope, sandbox_mongo::record_bson};

pub(crate) fn require_attachment(
    inbox: &StoredInbox,
    proof: &BridgeInvokeProof,
) -> Result<(), CallbackError> {
    proof
        .require_current()
        .map_err(|_| CallbackError::BindingUnavailable)?;
    if proof.effect()
        != &(BridgeInvokeEffect::AdmitMessage {
            inbox_id: inbox.id.clone(),
        })
        || proof.grant().owner != inbox.owner
        || proof.grant().pin != inbox.pin
        || proof.grant().config.connector.bot_id != inbox.bot_id
        || proof.grant().config.connector.key_id != inbox.key_id
        || inbox.admission != InboxAdmission::Accepted
    {
        return Err(CallbackError::BindingUnavailable);
    }
    inbox
        .text_attachment
        .as_ref()
        .ok_or(CallbackError::Payload)?
        .require_custody(inbox, DateTime::now().timestamp_millis() / 1000)
}

impl MongoCallbackInbox {
    pub(super) async fn retain_attachment(
        &self,
        write: &InboxWrite,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        match (&write.record().text_attachment, write.attachment_url()) {
            (None, None) => return Ok(()),
            (Some(metadata), Some(url)) => {
                metadata
                    .require_custody(write.record(), DateTime::now().timestamp_millis() / 1000)?;
                if metadata.captured.is_some()
                    || metadata.source_url_sha256 != format!("{:x}", Sha256::digest(url.as_bytes()))
                {
                    return Err(CallbackError::Payload.into());
                }
                let cipher = self
                    .cipher
                    .seal(
                        "bridge_text_attachment",
                        write.record().id.as_bytes(),
                        "url",
                        url,
                    )
                    .map_err(|_| CallbackError::Unavailable)?;
                self.attachments.insert_one(doc! {"_id": &write.record().id,
                    "expires_at": DateTime::from_millis(metadata.custody_expires_at.saturating_mul(1000)),
                    "source": record_bson(write.record()).map_err(|_| CallbackError::Unavailable)?,
                    "url": record_bson(&cipher).map_err(|_| CallbackError::Unavailable)?})
                    .session(session).await?;
            }
            _ => return Err(CallbackError::Payload.into()),
        }
        Ok(())
    }

    /// Requires current authority for this exact inbox/owner/Bridge/target and
    /// checks logical expiry before decryption, independently of Mongo TTL lag.
    pub(crate) async fn text_attachment_url(
        &self,
        inbox: &StoredInbox,
        proof: &BridgeInvokeProof,
        invokes: &crate::store::MongoBridgeInvokeStore,
    ) -> Result<Zeroizing<String>, CallbackError> {
        require_attachment(inbox, proof)?;
        invokes
            .authorize_text_attachment_read(proof, inbox)
            .await
            .map_err(|_| CallbackError::BindingUnavailable)?;
        let row = self
            .attachments
            .find_one(doc! {"_id": &inbox.id, "expires_at": {"$gt": DateTime::now()}})
            .await
            .map_err(|_| CallbackError::Unavailable)?
            .ok_or(CallbackError::Unavailable)?;
        let source: StoredInbox = mongodb::bson::deserialize_from_bson(
            row.get("source")
                .cloned()
                .ok_or(CallbackError::Unavailable)?,
        )
        .map_err(|_| CallbackError::Unavailable)?;
        if source != *inbox {
            return Err(CallbackError::ReplayConflict);
        }
        let envelope: CipherEnvelope = mongodb::bson::deserialize_from_bson(
            row.get("url").cloned().ok_or(CallbackError::Unavailable)?,
        )
        .map_err(|_| CallbackError::Unavailable)?;
        require_attachment(inbox, proof)?;
        let url = self
            .cipher
            .open(
                "bridge_text_attachment",
                inbox.id.as_bytes(),
                "url",
                envelope,
            )
            .map_err(|_| CallbackError::Unavailable)?;
        let url = Zeroizing::new(url);
        if inbox
            .text_attachment
            .as_ref()
            .is_none_or(|m| m.source_url_sha256 != format!("{:x}", Sha256::digest(url.as_bytes())))
        {
            return Err(CallbackError::ReplayConflict);
        }
        Ok(url)
    }
}
