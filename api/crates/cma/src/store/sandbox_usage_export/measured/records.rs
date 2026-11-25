use super::*;
use layer2_core::sandbox_metrics::Binding;
use mongodb::bson::{Bson, DateTime};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MeasuredStream {
    pub id: String,
    pub source_stream: String,
    pub source_epoch: String,
    pub namespace: String,
    pub owner: String,
    pub owner_issuer: String,
    pub sandbox_id: String,
    pub capture_start: u64,
    pub tracking_started_at: u64,
    pub next_minute: u64,
    pub replay_minute: u64,
    pub replay_turn: bool,
    pub sequence: u64,
    pub previous_digest: Option<String>,
}

impl MeasuredStream {
    pub fn key(&self) -> String {
        keys::stream(&self.namespace, &self.source_stream)
    }

    pub fn document(&self) -> Result<Document, SandboxStoreError> {
        Ok(
            doc! {"_id":keys::id(&self.key()), "document_schema_version":1_i64,
            "record":encode(self)?, "lease_holder":"", "lease_generation":0_i64,
            "lease_until":DateTime::from_millis(0)},
        )
    }

    pub fn read(row: &Document) -> Result<Self, SandboxStoreError> {
        let value: Self = decode(row)?;
        if keys::key(row)? != value.key()
            || [&value.id, &value.source_stream, &value.source_epoch]
                .into_iter()
                .any(|s| !uuid(s))
            || value.namespace.is_empty()
            || value.owner.is_empty()
            || value.owner_issuer.is_empty()
            || value.sandbox_id.is_empty()
            || value.tracking_started_at > value.capture_start
            || !value.capture_start.is_multiple_of(60)
            || !value.next_minute.is_multiple_of(60)
            || !value.replay_minute.is_multiple_of(60)
            || value.next_minute < value.capture_start
            || value.replay_minute < value.capture_start
            || value.replay_minute > value.next_minute
            || value.sequence > i64::MAX as u64
            || value.next_minute > i64::MAX as u64 / 1000 - 60
            || (value.sequence == 0) != value.previous_digest.is_none()
            || value.previous_digest.as_ref().is_some_and(|v| !digest(v))
        {
            return Err(corrupt());
        }
        Ok(value)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MeasuredLease {
    pub(super) stream: MeasuredStream,
    pub(super) holder: String,
    pub(super) generation: i64,
    pub(super) now: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FactHead {
    pub fact_id: String,
    pub revision: u64,
    pub event_id: String,
    pub digest: String,
    pub evidence_id: String,
    pub input_digest: String,
    pub resource: ResourceSnapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingProof {
    pub binding: Binding,
    pub resource: ResourceSnapshot,
    pub verified_from: u64,
    pub verified_until: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Authority {
    pub namespace: String,
    pub owner_issuer: String,
    pub capture_start: u64,
    pub after: Option<String>,
}

pub(super) fn document<T: Serialize>(key: &str, value: &T) -> Result<Document, SandboxStoreError> {
    Ok(doc! {"_id":keys::id(key),"document_schema_version":1_i64,"record":encode(value)?})
}

pub(super) fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|v| v.get_version_num() == 4 && v.to_string() == value)
}

pub(super) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn bytes(value: Vec<u8>) -> Bson {
    Bson::Binary(mongodb::bson::Binary {
        subtype: mongodb::bson::spec::BinarySubtype::Generic,
        bytes: value,
    })
}
