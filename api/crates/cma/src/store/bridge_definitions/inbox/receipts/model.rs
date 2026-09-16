//! Permanent acceptance identity and disposition, without retained payload.
use layer2_core::bridge::{
    ingress::{CallbackError, InboxAdmission, InboxReceipt, InboxWrite, StoredInbox},
    BridgeWorkPin,
};
use layer2_core::resource_access::ResourceOwner;
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};

use crate::store::sandbox_mongo::{read_record, record_bson, DOCUMENT_SCHEMA_VERSION};

pub(in crate::store) fn command_correlation(
    document: &Document,
) -> Result<Option<&Document>, CallbackError> {
    document
        .get("command_correlation")
        .map(|value| value.as_document().ok_or(CallbackError::Unavailable))
        .transpose()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct CommandCorrelation {
    pub command_id: String,
    pub response_id: String,
    pub allocation_id: String,
    pub body_sha256: String,
}
impl CommandCorrelation {
    pub(in crate::store) fn parse(value: &Document, body: &str) -> Result<Self, CallbackError> {
        let result: Self = mongodb::bson::deserialize_from_document(value.clone())
            .map_err(|_| CallbackError::Unavailable)?;
        if result.body_sha256 != body
            || [
                &result.command_id,
                &result.response_id,
                &result.allocation_id,
            ]
            .iter()
            .any(|id| id.is_empty() || id.len() > 256)
        {
            return Err(CallbackError::Unavailable);
        }
        Ok(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::store) enum AcceptanceDisposition {
    Pending,
    Command { correlation: CommandCorrelation },
    Expired { expired_at: i64 },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct AcceptedCallbackReceipt {
    pub receipt: InboxReceipt,
    pub owner: ResourceOwner,
    pub pin: BridgeWorkPin,
    pub bot_id: String,
    pub key_id: String,
    pub correlation_id: String,
    pub body_sha256: String,
    pub expires_at: i64,
    pub disposition: AcceptanceDisposition,
}
impl AcceptedCallbackReceipt {
    pub(in crate::store) fn capture(inbox: &StoredInbox) -> Result<Self, CallbackError> {
        let result = Self {
            receipt: inbox.receipt(),
            owner: inbox.owner.clone(),
            pin: inbox.pin.clone(),
            bot_id: inbox.bot_id.clone(),
            key_id: inbox.key_id.clone(),
            correlation_id: inbox.correlation_id.clone(),
            body_sha256: inbox.attestation.body_sha256.clone(),
            expires_at: inbox.expires_at,
            disposition: AcceptanceDisposition::Pending,
        };
        result.validate()?;
        Ok(result)
    }

    pub(in crate::store) fn require_source(
        &self,
        inbox: &StoredInbox,
    ) -> Result<(), CallbackError> {
        let mut expected = Self::capture(inbox)?;
        expected.disposition = self.disposition.clone();
        if self != &expected {
            return Err(CallbackError::ReplayConflict);
        }
        Ok(())
    }

    pub(in crate::store) fn replay(
        &self,
        write: &InboxWrite,
    ) -> Result<InboxReceipt, CallbackError> {
        write.require_current(write.bridge())?;
        self.validate()?;
        let input = write.record();
        if self.receipt.inbox_id != input.id
            || self.receipt.bridge_id != input.pin.bridge_id
            || self.receipt.message_id != input.message_id
            || self.owner != input.owner
            || self.bot_id != input.bot_id
            || self.key_id != input.key_id
            || self.correlation_id != input.correlation_id
            || self.body_sha256 != input.attestation.body_sha256
        {
            return Err(CallbackError::ReplayConflict);
        }
        // Returning the original immutable receipt never restores payload,
        // extends admission expiry, or authorizes a command/reply effect.
        Ok(self.receipt.clone())
    }

    pub(in crate::store) fn bind(&mut self, correlation: &Document) -> Result<(), CallbackError> {
        let correlation = CommandCorrelation::parse(correlation, &self.body_sha256)?;
        match &self.disposition {
            AcceptanceDisposition::Pending => {
                self.disposition = AcceptanceDisposition::Command { correlation };
                Ok(())
            }
            AcceptanceDisposition::Command { correlation: prior } if prior == &correlation => {
                Ok(())
            }
            _ => Err(CallbackError::ReplayConflict),
        }
    }

    pub(in crate::store) fn expire(&mut self, now: i64) -> Result<(), CallbackError> {
        if now < self.expires_at || !matches!(self.disposition, AcceptanceDisposition::Pending) {
            return Err(CallbackError::ReplayConflict);
        }
        self.disposition = AcceptanceDisposition::Expired {
            expired_at: self.expires_at,
        };
        Ok(())
    }

    fn validate(&self) -> Result<(), CallbackError> {
        if self.receipt.admission != InboxAdmission::Accepted
            || self.receipt.bridge_id != self.pin.bridge_id
            || self.receipt.inbox_id.is_empty()
            || self.receipt.message_id.is_empty()
            || self.expires_at <= self.receipt.accepted_at
            || self.body_sha256.len() != 64
            || !self
                .body_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || [&self.bot_id, &self.key_id, &self.correlation_id]
                .iter()
                .any(|v| v.is_empty() || v.len() > 256)
            || matches!(self.disposition, AcceptanceDisposition::Expired { expired_at } if expired_at != self.expires_at)
        {
            return Err(CallbackError::Unavailable);
        }
        if let AcceptanceDisposition::Command { correlation } = &self.disposition {
            let doc = mongodb::bson::serialize_to_document(correlation)
                .map_err(|_| CallbackError::Unavailable)?;
            CommandCorrelation::parse(&doc, &self.body_sha256)?;
        }
        Ok(())
    }

    pub(in crate::store) fn document(&self) -> Result<Document, CallbackError> {
        self.validate()?;
        let disposition = match self.disposition {
            AcceptanceDisposition::Pending => "pending",
            AcceptanceDisposition::Command { .. } => "command",
            AcceptanceDisposition::Expired { .. } => "expired",
        };
        Ok(
            doc! {"_id": &self.receipt.inbox_id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "bridge_id": self.receipt.bridge_id.as_str(), "disposition": disposition,
            "admission_expires_at": self.expires_at,
            "record": record_bson(self).map_err(|_| CallbackError::Unavailable)?},
        )
    }

    pub(in crate::store) fn read(document: &Document) -> Result<Self, CallbackError> {
        let value: Self = read_record(document).map_err(|_| CallbackError::Unavailable)?;
        let expected = value.document()?;
        if ["_id", "bridge_id", "disposition", "admission_expires_at"]
            .iter()
            .any(|key| expected.get(*key) != document.get(*key))
        {
            return Err(CallbackError::Unavailable);
        }
        Ok(value)
    }
}
