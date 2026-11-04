//! Bounded immutable derived-text chunks. Original binary bytes never enter the
//! record store; each chunk is bound to its parent, artifact and exact digest.
use super::{MediaArtifactClaim, MediaLease, MediaObligationStore, TextReference};
use crate::turns::routed::{storage, ExecutionError};
use cmaeg_core::media_continuation::{sha256, MAX_CONTINUATION_TEXT_BYTES};
use serde::{Deserialize, Serialize};
const CHUNK_BYTES: usize = 16 * 1024;
const KIND: &str = "routed_media_text_v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Chunk {
    kind: String,
    obligation: String,
    root: String,
    request_hash: String,
    artifact: String,
    digest: String,
    index: usize,
    count: usize,
    text: String,
}
impl MediaObligationStore {
    pub(super) async fn write_text(
        &self,
        lease: &MediaLease,
        artifact: &MediaArtifactClaim,
        text: &str,
    ) -> Result<TextReference, ExecutionError> {
        artifact
            .validate_derived_text(text)
            .map_err(|_| ExecutionError::Invalid)?;
        if text.len() > MAX_CONTINUATION_TEXT_BYTES {
            return Err(ExecutionError::Invalid);
        }
        let digest = sha256(text.as_bytes());
        let mut slices = Vec::new();
        let mut offset = 0;
        while offset < text.len() {
            let end = (offset + CHUNK_BYTES).min(text.len());
            let end = text.floor_char_boundary(end);
            slices.push(&text[offset..end]);
            offset = end;
        }
        let mut chunks = Vec::new();
        for (index, part) in slices.iter().enumerate() {
            self.guard(lease).await?;
            let id = storage::identifier(&(
                KIND,
                &lease.record.id,
                artifact.kind.as_str(),
                &digest,
                index,
            ));
            let chunk = Chunk {
                kind: KIND.into(),
                obligation: lease.record.id.clone(),
                root: lease.record.root.clone(),
                request_hash: lease.record.request_hash.clone(),
                artifact: artifact.kind.as_str().into(),
                digest: digest.clone(),
                index,
                count: slices.len(),
                text: (*part).into(),
            };
            self.0
                .write(
                    &id,
                    &lease.record.owner,
                    lease.record.retained_until,
                    &chunk,
                    None,
                )
                .await?;
            chunks.push(id);
        }
        Ok(TextReference {
            sha256: digest,
            len: text.len(),
            chunks,
        })
    }
    pub async fn derived_text(
        &self,
        lease: &MediaLease,
        artifact: &MediaArtifactClaim,
    ) -> Result<String, ExecutionError> {
        self.guard(lease).await?;
        let item = lease
            .record
            .supplements
            .iter()
            .find(|item| item.artifact == *artifact)
            .ok_or(ExecutionError::Invalid)?;
        let reference = &item.text;
        if reference.len > MAX_CONTINUATION_TEXT_BYTES
            || reference.chunks.len() > MAX_CONTINUATION_TEXT_BYTES / (CHUNK_BYTES - 3) + 1
        {
            return Err(ExecutionError::Corrupt);
        }
        let mut text = String::with_capacity(reference.len);
        for (index, id) in reference.chunks.iter().enumerate() {
            let expected = storage::identifier(&(
                KIND,
                &lease.record.id,
                artifact.kind.as_str(),
                &reference.sha256,
                index,
            ));
            if id != &expected {
                return Err(ExecutionError::Corrupt);
            }
            let (chunk, version) = self
                .0
                .read::<Chunk>(id)
                .await?
                .ok_or(ExecutionError::Corrupt)?;
            if version != 1
                || chunk.kind != KIND
                || chunk.obligation != lease.record.id
                || chunk.root != lease.record.root
                || chunk.request_hash != lease.record.request_hash
                || chunk.artifact != artifact.kind.as_str()
                || chunk.digest != reference.sha256
                || chunk.index != index
                || chunk.count != reference.chunks.len()
                || chunk.text.len() > CHUNK_BYTES
                || text.len() + chunk.text.len() > reference.len
            {
                return Err(ExecutionError::Corrupt);
            }
            text.push_str(&chunk.text);
        }
        if text.len() != reference.len || sha256(text.as_bytes()) != reference.sha256 {
            return Err(ExecutionError::Corrupt);
        }
        artifact
            .validate_derived_text(&text)
            .map_err(|_| ExecutionError::Corrupt)?;
        self.guard(lease).await?;
        Ok(text)
    }
}
