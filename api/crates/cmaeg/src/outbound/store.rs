//! Encrypted, domain-separated CAS records; message contents are never retained.
use super::{types::*, unavailable};
use crate::records::{RecordClient, RecordError};
use cmaeg_core::{
    ids::RecordId,
    message::RenderingReport,
    problem::{Problem, ProblemCode},
    profile::canonical_json,
    seal::{open_interaction, seal_interaction, MasterKeyRing},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const KIND: &str = "owner_outbound_v1";
pub(super) struct Store {
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Operation {
    pub kind: String,
    pub id: String,
    pub owner: String,
    pub record_id: String,
    pub authority_hash: String,
    pub request_hash: String,
    pub message_hash: String,
    pub receipt: OutboundReceipt,
    #[serde(skip)]
    pub version: u64,
}
impl Store {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            records: records.interactions(),
            keys,
        }
    }
    pub async fn load(&self, id: &str) -> Result<Operation, Problem> {
        let url = self.records.record_url(id).map_err(|_| not_found())?;
        let value = self.records.get(url.as_str()).await.map_err(|e| match e {
            RecordError::NotFound => not_found(),
            _ => unavailable(),
        })?;
        let opened = open_interaction(&self.keys, id, &value.blob).map_err(|_| not_found())?;
        let mut op: Operation = serde_json::from_slice(&opened.payload).map_err(|_| not_found())?;
        if op.kind != KIND || op.id != id || op.receipt.delivery_id != id {
            return Err(not_found());
        }
        op.version = value.version;
        Ok(op)
    }
    pub async fn save(&self, op: &Operation, expected: Option<u64>) -> Result<u64, Problem> {
        let bytes = serde_json::to_vec(op).map_err(|_| unavailable())?;
        let blob = seal_interaction(&self.keys, &op.id, &bytes).map_err(|_| unavailable())?;
        self.records
            .put_expiring(
                &RecordId::new(&op.id).map_err(|_| unavailable())?,
                &op.owner,
                &blob,
                expected,
                Some(op.receipt.retained_until),
            )
            .await
            .map_err(|e| match e {
                RecordError::Conflict { .. } => {
                    Problem::new(ProblemCode::Conflict, 409, "outbound_operation_changed")
                }
                _ => unavailable(),
            })
    }
}
impl Operation {
    pub fn identifier(owner: &str, record_id: &str, key: &str) -> String {
        let digest = Sha256::digest(format!(
            "cmaeg.owner-outbound.v1\0{owner}\0{record_id}\0{key}"
        ));
        let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        uuid::Uuid::from_bytes(bytes).to_string()
    }
    pub fn new(
        owner: &str,
        record_id: &str,
        authority_hash: String,
        key: &str,
        request: &PrepareOutboundMessage,
        rendering: RenderingReport,
        now: i64,
    ) -> Self {
        let id = Self::identifier(owner, record_id, key);
        Self {
            kind: KIND.into(),
            id: id.clone(),
            owner: owner.into(),
            record_id: record_id.into(),
            authority_hash,
            request_hash: hash(request),
            message_hash: hash(&request.message),
            version: 0,
            receipt: OutboundReceipt {
                delivery_id: id,
                source_id: request.source_id.clone(),
                route_id: request.route_id.clone(),
                status: OutboundStatus::Prepared,
                expires_at: now.saturating_add(86400),
                retained_until: now.saturating_add(7 * 86400),
                rendering,
                delivery: None,
            },
        }
    }
    pub fn public(&self) -> OutboundReceipt {
        let mut receipt = self.receipt.clone();
        if receipt.status == OutboundStatus::Committed && receipt.delivery.is_none() {
            receipt.delivery = Some(crate::delivery::intents::DeliveryOutcome::Unknown {
                reason: "delivery_in_progress_or_uncertain".into(),
            });
        }
        receipt
    }
}
pub(super) fn hash(value: &impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            canonical_json(serde_json::to_value(value).expect("wire value serializes")).as_bytes()
        )
    )
}
pub(super) fn not_found() -> Problem {
    Problem::new(
        ProblemCode::InvalidRequest,
        404,
        "outbound_operation_not_found",
    )
}
