//! Permanent provider identity and exact alias source, without message content.
use layer2_core::{
    bridge::{
        ingress::{CallbackError, InboxReceipt, InboxWrite, StoredInbox},
        BridgeWorkPin,
    },
    resource_access::ResourceOwner,
};
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};

use super::super::receipts::model::AcceptedCallbackReceipt;
use crate::store::sandbox_mongo::{read_record, record_bson, DOCUMENT_SCHEMA_VERSION};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    inbox_id: String,
    owner: ResourceOwner,
    pin: BridgeWorkPin,
    bot_id: String,
    key_id: String,
    message_id: String,
    correlation_id: String,
    issuer: String,
    body_sha256: String,
}
impl Source {
    fn capture(inbox: &StoredInbox) -> Self {
        Self {
            inbox_id: inbox.id.clone(),
            owner: inbox.owner.clone(),
            pin: inbox.pin.clone(),
            bot_id: inbox.bot_id.clone(),
            key_id: inbox.key_id.clone(),
            message_id: inbox.message_id.clone(),
            correlation_id: inbox.correlation_id.clone(),
            issuer: inbox.attestation.issuer.clone(),
            body_sha256: inbox.attestation.body_sha256.clone(),
        }
    }
    fn require_exact(&self, write: &InboxWrite) -> Result<(), CallbackError> {
        let mut expected = Self::capture(write.record());
        // As with ordinary receipt replay, a current revision does not rewrite history.
        expected.pin.config_revision = self.pin.config_revision;
        if self != &expected {
            return Err(CallbackError::ReplayConflict);
        }
        Ok(())
    }
    fn valid(&self) -> bool {
        hash(&self.inbox_id)
            && hash(&self.body_sha256)
            && self.pin.config_revision > 0
            && [
                &self.bot_id,
                &self.key_id,
                &self.message_id,
                &self.correlation_id,
                &self.issuer,
            ]
            .iter()
            .all(|value| {
                !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
            })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FirstMessage {
    key_sha256: String,
    semantic_sha256: String,
    source: Source,
    receipt: InboxReceipt,
    expires_at: i64,
}
impl FirstMessage {
    pub(super) fn capture(write: &InboxWrite) -> Result<Self, CallbackError> {
        let proof = write
            .platform_redelivery()
            .ok_or(CallbackError::Unavailable)?;
        let accepted = AcceptedCallbackReceipt::capture(write.record())?;
        Ok(Self {
            key_sha256: proof.key_sha256().into(),
            semantic_sha256: proof.semantic_sha256().into(),
            source: Source::capture(write.record()),
            receipt: accepted.receipt,
            expires_at: accepted.expires_at,
        })
    }
    pub(super) fn require_equivalent(&self, write: &InboxWrite) -> Result<(), CallbackError> {
        let proof = write
            .platform_redelivery()
            .ok_or(CallbackError::ReplayConflict)?;
        let input = write.record();
        if self.key_sha256 != proof.key_sha256()
            || self.semantic_sha256 != proof.semantic_sha256()
            || self.source.owner != input.owner
            || self.source.pin.bridge_id != input.pin.bridge_id
            || self.source.bot_id != input.bot_id
            || self.source.issuer != input.attestation.issuer
        {
            return Err(CallbackError::ReplayConflict);
        }
        Ok(())
    }
    fn identity(&self) -> Result<String, CallbackError> {
        layer2_core::profiles::content_digest(self).map_err(|_| CallbackError::Unavailable)
    }
    pub(super) fn receipt_id(&self) -> &str {
        &self.source.inbox_id
    }
    pub(super) fn require_receipt(
        &self,
        receipt: &AcceptedCallbackReceipt,
    ) -> Result<InboxReceipt, CallbackError> {
        // Disposition can advance to Command or Expired; accepted source and route cannot.
        if receipt.receipt != self.receipt
            || receipt.owner != self.source.owner
            || receipt.pin != self.source.pin
            || receipt.bot_id != self.source.bot_id
            || receipt.key_id != self.source.key_id
            || receipt.correlation_id != self.source.correlation_id
            || receipt.body_sha256 != self.source.body_sha256
            || receipt.expires_at != self.expires_at
        {
            return Err(CallbackError::Unavailable);
        }
        Ok(receipt.receipt.clone())
    }
    pub(super) fn document(&self) -> Result<Document, CallbackError> {
        if !hash(&self.key_sha256)
            || !hash(&self.semantic_sha256)
            || !self.source.valid()
            || self.receipt.inbox_id != self.source.inbox_id
            || self.receipt.message_id != self.source.message_id
            || self.receipt.bridge_id != self.source.pin.bridge_id
            || self.expires_at <= self.receipt.accepted_at
            || self.receipt.admission != layer2_core::bridge::ingress::InboxAdmission::Accepted
        {
            return Err(CallbackError::Unavailable);
        }
        Ok(
            doc! {"_id":first_id(&self.key_sha256),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "kind":"first_message","record_sha256":self.identity()?,"record":record_bson(self).map_err(|_| CallbackError::Unavailable)?},
        )
    }
    pub(super) fn read(document: &Document, key: &str) -> Result<Self, CallbackError> {
        let value: Self = read_record(document).map_err(|_| CallbackError::Unavailable)?;
        let expected = value.document()?;
        if value.key_sha256 != key
            || expected.get("record_sha256") != document.get("record_sha256")
            || expected.get("_id") != document.get("_id")
            || document.get_str("kind").ok() != Some("first_message")
        {
            return Err(CallbackError::Unavailable);
        }
        Ok(value)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MessageAlias {
    first_sha256: String,
    key_sha256: String,
    semantic_sha256: String,
    source: Source,
}
impl MessageAlias {
    pub(super) fn capture(write: &InboxWrite, first: &FirstMessage) -> Result<Self, CallbackError> {
        let proof = write
            .platform_redelivery()
            .ok_or(CallbackError::Unavailable)?;
        Ok(Self {
            first_sha256: first.identity()?,
            key_sha256: proof.key_sha256().into(),
            semantic_sha256: proof.semantic_sha256().into(),
            source: Source::capture(write.record()),
        })
    }
    pub(super) fn require_first(&self, first: &FirstMessage) -> Result<(), CallbackError> {
        if self.first_sha256 != first.identity()? {
            return Err(CallbackError::Unavailable);
        }
        Ok(())
    }
    pub(super) fn key(&self) -> &str {
        &self.key_sha256
    }
    pub(super) fn require_exact(&self, write: &InboxWrite) -> Result<(), CallbackError> {
        self.source.require_exact(write)?;
        let proof = write
            .platform_redelivery()
            .ok_or(CallbackError::ReplayConflict)?;
        if self.key_sha256 != proof.key_sha256() || self.semantic_sha256 != proof.semantic_sha256()
        {
            return Err(CallbackError::ReplayConflict);
        }
        Ok(())
    }
    pub(super) fn document(&self) -> Result<Document, CallbackError> {
        if !hash(&self.first_sha256)
            || !hash(&self.key_sha256)
            || !hash(&self.semantic_sha256)
            || !self.source.valid()
        {
            return Err(CallbackError::Unavailable);
        }
        Ok(
            doc! {"_id":alias_id(&self.source.inbox_id),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "kind":"alias","record_sha256":layer2_core::profiles::content_digest(self).map_err(|_| CallbackError::Unavailable)?,"record":record_bson(self).map_err(|_| CallbackError::Unavailable)?},
        )
    }
    pub(super) fn read(document: &Document) -> Result<Self, CallbackError> {
        let value: Self = read_record(document).map_err(|_| CallbackError::Unavailable)?;
        let expected = value.document()?;
        if expected.get("_id") != document.get("_id")
            || expected.get("record_sha256") != document.get("record_sha256")
            || document.get_str("kind").ok() != Some("alias")
        {
            return Err(CallbackError::Unavailable);
        }
        Ok(value)
    }
}

pub(super) fn first_id(key: &str) -> String {
    format!("message:{key}")
}
pub(super) fn alias_id(inbox: &str) -> String {
    format!("alias:{inbox}")
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
