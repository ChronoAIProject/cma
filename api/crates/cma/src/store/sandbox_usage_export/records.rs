use super::*;
use layer2_core::sandbox_usage::UsageEvent;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stream {
    pub id: String,
    pub source_epoch: String,
    pub namespace: String,
    pub owner: String,
    pub owner_issuer: String,
    pub sandbox_id: String,
    pub created_at: u64,
    pub tracking_started_at: u64,
    pub capture_start: u64,
    pub first_revision: u64,
    pub last_revision: u64,
    pub last_ended_at: u64,
    pub complete_lifecycle: bool,
    pub sequence: u64,
    pub previous_digest: Option<String>,
    pub projected_revision: Option<u64>,
    pub resource: ResourceSnapshot,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResourceSnapshot {
    pub runtime_id: Option<String>,
    pub runtime_generation: Option<u64>,
    pub home_id: Option<String>,
    pub home_generation: Option<String>,
}

impl ResourceSnapshot {
    pub fn from_sandbox(sandbox: &layer2_core::sandbox::Sandbox) -> Self {
        let home = sandbox
            .lifecycle
            .home
            .as_ref()
            .filter(|h| !h.released)
            .and_then(|h| h.receipt.as_ref());
        Self {
            runtime_id: sandbox.runtime.as_ref().map(|r| r.runtime_id.clone()),
            runtime_generation: sandbox
                .runtime
                .as_ref()
                .and_then(|r| r.provision_generation),
            home_id: home.map(|h| h.volume_id.clone()),
            home_generation: home.map(|h| h.storage_generation.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Captured {
    pub stream_id: String,
    pub source: UsageEvent,
    pub previous_revision: Option<u64>,
    pub source_gap: bool,
    pub tracking_started_at: u64,
    pub complete_lifecycle: bool,
    pub resource: ResourceSnapshot,
}

pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn encode<T: Serialize>(record: &T) -> Result<mongodb::bson::Bson, SandboxStoreError> {
    mongodb::bson::serialize_to_bson(record).map_err(|_| corrupt())
}

pub(super) fn decode<T: DeserializeOwned>(document: &Document) -> Result<T, SandboxStoreError> {
    if document.get_i64("document_schema_version").ok() != Some(1) {
        return Err(corrupt());
    }
    mongodb::bson::deserialize_from_bson(document.get("record").cloned().ok_or_else(corrupt)?)
        .map_err(|_| corrupt())
}

pub(super) fn number(value: u64) -> Result<i64, SandboxStoreError> {
    i64::try_from(value).map_err(|_| corrupt())
}

impl Stream {
    pub fn document(&self) -> Result<Document, SandboxStoreError> {
        Ok(doc! {"_id":&self.id, "document_schema_version":1_i64,
        "namespace":&self.namespace, "owner":&self.owner, "sandbox_id":&self.sandbox_id,
        "created_at":number(self.created_at)?, "record":encode(self)?})
    }

    pub fn read(document: &Document) -> Result<Self, SandboxStoreError> {
        let stream: Self = decode(document)?;
        if stream.document()? != *document {
            return Err(corrupt());
        }
        if stream.created_at > stream.tracking_started_at
            || stream.tracking_started_at > stream.capture_start
            || stream.capture_start > stream.last_ended_at
            || stream.last_ended_at > i64::MAX as u64 / 1000
            || stream.first_revision == 0
            || stream.first_revision > stream.last_revision
            || stream.last_revision > i64::MAX as u64
            || stream.sequence > i64::MAX as u64
            || (stream.sequence == 0) != stream.previous_digest.is_none()
            || stream.previous_digest.as_ref().is_some_and(|digest| {
                digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
            || stream.projected_revision.is_some_and(|revision| {
                revision < stream.first_revision
                    || revision > stream.last_revision
                    || stream.sequence < 2
            })
        {
            return Err(corrupt());
        }
        for id in [&stream.id, &stream.source_epoch] {
            if uuid::Uuid::parse_str(id)
                .ok()
                .is_none_or(|id| id.get_version_num() != 4)
            {
                return Err(corrupt());
            }
        }
        Ok(stream)
    }
}
