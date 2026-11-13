//! One unresolved result per original event/consumer, independent of request keys.
use super::{receipts::hash, unavailable};
use crate::records::{RecordClient, RecordError};
use cmaeg_core::{
    problem::Problem,
    seal::{
        open_interaction, seal_interaction, MasterKeyRing, ReplyTargetPayload, REPLY_TARGET_TTL,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fence {
    kind: String,
    id: String,
    owner: String,
    scope: String,
    expires_at: i64,
    active: bool,
    #[serde(skip)]
    version: u64,
}

impl Fence {
    pub(super) async fn acquire(
        records: &RecordClient,
        keys: &MasterKeyRing,
        target: &ReplyTargetPayload,
        owner: &str,
    ) -> Result<Option<Self>, Problem> {
        // Issuance time, token ciphertext, message and caller key cannot create
        // another uncertainty domain for the same original event and consumer.
        let scope = hash(&serde_json::json!({"record": target.event.record_id,
            "event": target.event.event_id, "consumer": target.event.consumer}));
        let digest = Sha256::digest(format!("cmaeg.reply-uncertainty.v1\0{scope}"));
        let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let mut fence = Self {
            kind: "reply_uncertainty_v1".into(),
            id: uuid::Uuid::from_bytes(bytes).to_string(),
            owner: owner.into(),
            scope,
            expires_at: target.event.expires_at.saturating_add(REPLY_TARGET_TTL),
            active: true,
            version: 0,
        };
        let url = records.record_url(&fence.id).map_err(|_| unavailable())?;
        let expected = match records.get(url.as_str()).await {
            Ok(row) => {
                let opened =
                    open_interaction(keys, &fence.id, &row.blob).map_err(|_| unavailable())?;
                let saved: Self =
                    serde_json::from_slice(&opened.payload).map_err(|_| unavailable())?;
                if saved.kind != fence.kind
                    || saved.id != fence.id
                    || saved.owner != fence.owner
                    || saved.scope != fence.scope
                    || saved.expires_at != fence.expires_at
                {
                    return Err(unavailable());
                }
                if saved.active {
                    return Ok(None);
                }
                Some(row.version)
            }
            Err(RecordError::NotFound) => None,
            Err(_) => return Err(unavailable()),
        };
        match fence.save(records, keys, expected).await {
            Ok(version) => {
                fence.version = version;
                Ok(Some(fence))
            }
            // The winner may already be sending. A conflict never authorizes retry.
            Err(RecordError::Conflict { .. }) => Ok(None),
            Err(_) => Err(unavailable()),
        }
    }

    /// Only called after a definite result and its exact receipt are durable.
    pub(super) async fn release(
        mut self,
        records: &RecordClient,
        keys: &MasterKeyRing,
    ) -> Result<(), Problem> {
        self.active = false;
        self.save(records, keys, Some(self.version))
            .await
            .map_err(|_| unavailable())?;
        Ok(())
    }

    async fn save(
        &self,
        records: &RecordClient,
        keys: &MasterKeyRing,
        expected: Option<u64>,
    ) -> Result<u64, RecordError> {
        let payload =
            serde_json::to_vec(self).map_err(|error| RecordError::Malformed(error.to_string()))?;
        let blob = seal_interaction(keys, &self.id, &payload)
            .map_err(|error| RecordError::Malformed(error.to_string()))?;
        records
            .put_expiring(
                &self
                    .id
                    .parse()
                    .map_err(|_| RecordError::Malformed("invalid fence ID".into()))?,
                &self.owner,
                &blob,
                expected,
                Some(self.expires_at),
            )
            .await
    }
}
