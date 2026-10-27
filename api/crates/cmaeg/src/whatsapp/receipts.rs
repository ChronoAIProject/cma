//! Confirmed native outbound targets, sealed in existing CMA interaction storage.
use crate::{
    ingress::routing_store::EventIdentity,
    records::{RecordClient, RecordError},
};
use cmaeg_core::{
    activity::SourceKind,
    channel::ProviderBinding,
    ids::{ChannelId, ConsumerId, ConversationId, RecordId},
    seal::{open_interaction, seal_interaction, MasterKeyRing},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const RETENTION: i64 = 7 * 86400;
#[derive(Clone)]
pub struct Store {
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub kind: String,
    pub original: EventIdentity,
    pub source: SourceKind,
    pub recipient: String,
    pub message_id: String,
    pub consumer: ConsumerId,
    pub provider: ProviderBinding,
    pub partition_key: String,
    pub conversation_id: ConversationId,
    pub confirmed_at: i64,
    pub retained_until: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Conflict,
    Invalid,
    Expired,
}

impl Store {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            records: records.interactions(),
            keys,
        }
    }
    fn id(channel: &ChannelId, source: &SourceKind, recipient: &str, message: &str) -> String {
        let hash = Sha256::digest(
            serde_json::to_vec(&(
                "native_whatsapp_receipt_v1",
                channel,
                source,
                recipient,
                message,
            ))
            .expect("receipt identity serializes"),
        );
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&hash[..16]);
        bytes[6] = (bytes[6] & 15) | 64;
        bytes[8] = (bytes[8] & 63) | 128;
        uuid::Uuid::from_bytes(bytes).to_string()
    }
    pub(crate) async fn put(&self, receipt: &Receipt) -> Result<(), Error> {
        receipt.validate(receipt.confirmed_at)?;
        let id = Self::id(
            &receipt.original.channel_id,
            &receipt.source,
            &receipt.recipient,
            &receipt.message_id,
        );
        let bytes = serde_json::to_vec(receipt).map_err(|_| Error::Invalid)?;
        let blob = seal_interaction(&self.keys, &id, &bytes).map_err(|_| Error::Invalid)?;
        let record = RecordId::new(&id).map_err(|_| Error::Invalid)?;
        match self
            .records
            .put_expiring(
                &record,
                &receipt.original.owner,
                &blob,
                None,
                Some(receipt.retained_until),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(RecordError::Conflict { .. } | RecordError::Unavailable(_)) => {
                match self
                    .load(
                        &receipt.original.channel_id,
                        &receipt.source,
                        &receipt.recipient,
                        &receipt.message_id,
                        receipt.confirmed_at,
                    )
                    .await?
                {
                    Some(existing) if existing == *receipt => Ok(()),
                    Some(_) => Err(Error::Conflict),
                    None => Err(Error::Unavailable),
                }
            }
            Err(_) => Err(Error::Unavailable),
        }
    }
    pub(crate) async fn load(
        &self,
        channel: &ChannelId,
        source: &SourceKind,
        recipient: &str,
        message: &str,
        now: i64,
    ) -> Result<Option<Receipt>, Error> {
        let id = Self::id(channel, source, recipient, message);
        let url = self.records.record_url(&id).map_err(|_| Error::Invalid)?;
        let value = match self.records.get(url.as_str()).await {
            Ok(value) => value,
            Err(RecordError::NotFound) => return Ok(None),
            Err(_) => return Err(Error::Unavailable),
        };
        if value.version != 1 {
            return Err(Error::Invalid);
        }
        let opened = open_interaction(&self.keys, &id, &value.blob).map_err(|_| Error::Invalid)?;
        let receipt: Receipt =
            serde_json::from_slice(&opened.payload).map_err(|_| Error::Invalid)?;
        receipt.validate(now)?;
        if receipt.original.channel_id != *channel
            || receipt.source != *source
            || receipt.recipient != recipient
            || receipt.message_id != message
        {
            return Err(Error::Invalid);
        }
        Ok(Some(receipt))
    }
}
impl Receipt {
    fn validate(&self, now: i64) -> Result<(), Error> {
        if self.kind != "native_whatsapp_receipt_v1"
            || !matches!(self.source, SourceKind::Whatsapp { .. })
            || !cmaeg_core::channel::whatsapp::valid_message_id(&self.message_id)
            || !cmaeg_core::channel::whatsapp::valid_meta_id(&self.recipient)
            || self.original.owner.is_empty()
            || self.confirmed_at <= 0
            || self.retained_until <= self.confirmed_at
            || self.retained_until - self.confirmed_at > RETENTION
        {
            return Err(Error::Invalid);
        }
        if self.retained_until <= now {
            return Err(Error::Expired);
        }
        Ok(())
    }
}
