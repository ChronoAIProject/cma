//! A durable claim precedes every external send. An abandoned claim is never resent.
use super::unavailable;
use crate::{
    delivery::intents::{DeliveryOutcome, IntentReceipt},
    records::{RecordClient, RecordError},
};
use cmaeg_core::{
    ids::RecordId,
    message::{OutboundMessage, RenderingOutcome, RenderingReport},
    problem::{Problem, ProblemCode},
    profile::canonical_json,
    seal::{
        open_interaction, seal_interaction, MasterKeyRing, ReplyTargetPayload, REPLY_TARGET_TTL,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub(super) struct Store {
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Claim {
    kind: String,
    id: String,
    owner: String,
    scope: String,
    request_hash: String,
    expires_at: i64,
    receipt: Option<IntentReceipt>,
    #[serde(skip)]
    version: u64,
}
pub(super) enum Admission {
    Claimed(Claim),
    Replay(IntentReceipt),
}
impl Store {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            records: records.interactions(),
            keys,
        }
    }
    pub async fn claim(
        &self,
        target: &ReplyTargetPayload,
        request_id: &str,
        message: &OutboundMessage,
        owner: &str,
    ) -> Result<Admission, Problem> {
        // Exclude randomized token ciphertext and issuance time so reminting cannot bypass dedupe.
        let scope = hash(&serde_json::json!({"channel":target.event.channel_id,
            "record":target.event.record_id,"provider":target.provider,"binding":target.binding_id,
            "key":target.agent_key_hash,"event":target.event}));
        let digest =
            Sha256::digest(format!("cmaeg.reply-delivery.v1\0{scope}\0{request_id}").as_bytes());
        let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let mut claim = Claim {
            kind: "reply_delivery_v1".into(),
            id: uuid::Uuid::from_bytes(bytes).to_string(),
            owner: owner.into(),
            scope,
            request_hash: hash(message),
            // Retain through every target that can be minted from this original event.
            expires_at: target.event.expires_at.saturating_add(REPLY_TARGET_TTL),
            receipt: None,
            version: 0,
        };
        let url = self
            .records
            .record_url(&claim.id)
            .map_err(|_| unavailable())?;
        match self.records.get(url.as_str()).await {
            Ok(value) => return self.replay(&claim, &value.blob),
            Err(RecordError::NotFound) => {}
            Err(_) => return Err(unavailable()),
        }
        match self.save(&claim, None).await {
            Ok(version) => {
                claim.version = version;
                Ok(Admission::Claimed(claim))
            }
            Err(RecordError::Conflict { .. }) => {
                let value = self
                    .records
                    .get(url.as_str())
                    .await
                    .map_err(|_| unavailable())?;
                self.replay(&claim, &value.blob)
            }
            Err(_) => Err(unavailable()),
        }
    }
    fn replay(&self, expected: &Claim, blob: &[u8]) -> Result<Admission, Problem> {
        let opened = open_interaction(&self.keys, &expected.id, blob).map_err(|_| unavailable())?;
        let saved: Claim = serde_json::from_slice(&opened.payload).map_err(|_| unavailable())?;
        if saved.kind != expected.kind
            || saved.id != expected.id
            || saved.owner != expected.owner
            || saved.scope != expected.scope
            || saved.request_hash != expected.request_hash
        {
            return Err(Problem::new(
                ProblemCode::Conflict,
                409,
                "reply_idempotency_conflict",
            ));
        }
        Ok(Admission::Replay(saved.receipt.unwrap_or_else(|| {
            uncertain("delivery_in_progress_or_uncertain")
        })))
    }
    pub async fn finish(&self, mut claim: Claim, receipt: &IntentReceipt) -> Result<(), Problem> {
        claim.receipt = Some(receipt.clone());
        self.save(&claim, Some(claim.version))
            .await
            .map_err(|_| unavailable())?;
        Ok(())
    }
    async fn save(&self, claim: &Claim, expected: Option<u64>) -> Result<u64, RecordError> {
        let bytes = serde_json::to_vec(claim).map_err(|e| RecordError::Malformed(e.to_string()))?;
        let blob = seal_interaction(&self.keys, &claim.id, &bytes)
            .map_err(|e| RecordError::Malformed(e.to_string()))?;
        self.records
            .put_expiring(
                &RecordId::new(&claim.id).map_err(|e| RecordError::Malformed(e.to_string()))?,
                &claim.owner,
                &blob,
                expected,
                Some(claim.expires_at),
            )
            .await
    }
}
fn hash(value: &impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            canonical_json(serde_json::to_value(value).expect("wire value serializes")).as_bytes()
        )
    )
}
pub(in crate::tools) fn uncertain(reason: &str) -> IntentReceipt {
    IntentReceipt {
        message_ref: None,
        rendering: RenderingReport {
            outcome: RenderingOutcome::Unsupported,
            blocks: vec![],
        },
        delivery: DeliveryOutcome::Unknown {
            reason: reason.into(),
        },
    }
}
