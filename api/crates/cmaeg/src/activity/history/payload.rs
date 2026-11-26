use super::{repository::*, Error};
use crate::activity::ChannelActivity;
use cmaeg_core::seal::{open_interaction, seal_interaction, MasterKeyRing};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) const INLINE_BYTES: usize = 24 * 1024;
const CHUNK_BYTES: usize = 32 * 1024;
const MAX_CHUNKS: usize = 32;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Part {
    pub id: String,
    pub hash: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub generation: String,
    pub parts: Vec<Part>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Root {
    pub schema: u8,
    pub scope: String,
    pub event_key: String,
    pub received_ms: i64,
    pub expires_ms: i64,
    pub receipt: ChannelActivity,
    pub inline: Option<ChannelActivity>,
    pub manifest: Option<Manifest>,
    pub legacy_partial: bool,
    pub detail_incomplete: bool,
    pub detail_truncated: bool,
}
impl Root {
    pub fn encode(&self, keys: &MasterKeyRing) -> Result<Vec<u8>, Error> {
        let bytes = serde_json::to_vec(self).map_err(|_| Error::Unreadable)?;
        if bytes.len() > 44_000 {
            return Err(Error::DetailLimit);
        }
        seal_interaction(keys, &self.event_key, &bytes).map_err(|_| Error::Unreadable)
    }
    pub fn decode(keys: &MasterKeyRing, stored: &StoredEvent) -> Result<Self, Error> {
        if stored.blob.len() > 65536 {
            return Err(Error::Unreadable);
        }
        let opened = open_interaction(keys, &stored.key.event_key, &stored.blob)
            .map_err(|_| Error::Unreadable)?;
        let root: Self = serde_json::from_slice(&opened.payload).map_err(|_| Error::Unreadable)?;
        if root.schema != 1
            || root.scope != stored.scope
            || root.event_key != stored.key.event_key
            || root.received_ms != stored.key.received_ms
            || root.expires_ms != stored.expires_ms
            || super::event_key(&root.scope, &root.receipt.event_id) != root.event_key
            || root
                .manifest
                .as_ref()
                .is_some_and(|m| m.parts.len() > MAX_CHUNKS || m.parts.is_empty())
            || root.inline.is_some() == root.manifest.is_some()
        {
            return Err(Error::Unreadable);
        }
        Ok(root)
    }
    pub async fn summary(
        &self,
        repo: &dyn ActivityHistoryRepository,
        keys: &MasterKeyRing,
        now_ms: i64,
    ) -> Result<ChannelActivity, Error> {
        if let Some(inline) = &self.inline {
            return Ok(inline.clone());
        }
        let manifest = self.manifest.as_ref().ok_or(Error::Unreadable)?;
        let mut bytes = Vec::new();
        for part in &manifest.parts {
            bytes.extend(
                read_part(
                    repo,
                    keys,
                    &self.scope,
                    &self.event_key,
                    self.expires_ms,
                    part,
                    now_ms,
                )
                .await?,
            );
        }
        let summary: ChannelActivity =
            serde_json::from_slice(&bytes).map_err(|_| Error::Unreadable)?;
        if summary.event_id != self.receipt.event_id
            || summary.received_at != self.receipt.received_at
        {
            return Err(Error::Unreadable);
        }
        Ok(summary)
    }
    pub async fn project(
        &mut self,
        summary: ChannelActivity,
        repo: &dyn ActivityHistoryRepository,
        keys: &MasterKeyRing,
    ) -> Result<(), Error> {
        let bytes = serde_json::to_vec(&summary).map_err(|_| Error::Unreadable)?;
        if bytes.len() > CHUNK_BYTES * MAX_CHUNKS {
            return Err(Error::DetailLimit);
        }
        if bytes.len() <= INLINE_BYTES {
            self.inline = Some(summary);
            self.manifest = None;
            return Ok(());
        }
        let generation = uuid::Uuid::new_v4().to_string();
        let mut parts = Vec::new();
        for (index, bytes) in bytes.chunks(CHUNK_BYTES).enumerate() {
            let id = super::digest(&[
                "activity-history-detail-v1",
                &self.event_key,
                &generation,
                &index.to_string(),
            ]);
            let blob = seal_interaction(keys, &id, bytes).map_err(|_| Error::DetailLimit)?;
            repo.put_chunk(StoredChunk {
                id: id.clone(),
                scope: self.scope.clone(),
                event_key: self.event_key.clone(),
                expires_ms: self.expires_ms,
                blob,
            })
            .await?;
            parts.push(Part {
                id,
                hash: hash(bytes),
            });
        }
        // Publication happens only after all immutable parts exist. Losing
        // generations expire at the event's original expiry without a sweep.
        self.inline = None;
        self.manifest = Some(Manifest { generation, parts });
        Ok(())
    }
}
pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) async fn read_part(
    repo: &dyn ActivityHistoryRepository,
    keys: &MasterKeyRing,
    scope: &str,
    event: &str,
    expiry: i64,
    part: &Part,
    now_ms: i64,
) -> Result<Vec<u8>, Error> {
    let chunk = repo
        .chunk(scope, &part.id, now_ms)
        .await?
        .ok_or(Error::Unreadable)?;
    if chunk.event_key != event || chunk.expires_ms != expiry || chunk.blob.len() > 65536 {
        return Err(Error::Unreadable);
    }
    let opened = open_interaction(keys, &part.id, &chunk.blob).map_err(|_| Error::Unreadable)?;
    if opened.payload.len() > CHUNK_BYTES || hash(&opened.payload) != part.hash {
        return Err(Error::Unreadable);
    }
    Ok(opened.payload)
}
