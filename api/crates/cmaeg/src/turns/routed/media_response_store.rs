//! Independent durable identity and delivery outcomes for each appended media
//! response. Original event delivery state is never reopened or overwritten.
use super::{storage, CustomCheckpoint, ExecutionError, MediaLease, MediaObligationStore};
use crate::delivery::intents::DeliveryOutcome;
use cmaeg_core::media_continuation::{sha256, MediaContinuationKind};
use serde::{Deserialize, Serialize};
const KIND: &str = "routed_media_response_v1";
const MAX_TEXT: usize = 64 * 1024;
const CHUNK: usize = 16 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MediaResponseState {
    kind: String,
    pub id: String,
    obligation: String,
    pub response_id: String,
    pub artifact: MediaContinuationKind,
    pub status: String,
    pub text_sha256: String,
    pub text_len: usize,
    pub text_chunks: Vec<String>,
    pub custom: Option<CustomCheckpoint>,
    pub native: NativeOutcome,
    #[serde(skip)]
    pub version: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum NativeOutcome {
    NotStarted,
    Intent,
    Settled { outcome: DeliveryOutcome },
    Unknown,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TextChunk {
    kind: String,
    response: String,
    index: usize,
    digest: String,
    text: String,
}
impl MediaObligationStore {
    pub(super) async fn media_response(
        &self,
        lease: &MediaLease,
        kind: MediaContinuationKind,
    ) -> Result<Option<MediaResponseState>, ExecutionError> {
        self.guard(lease).await?;
        self.retained_media_response(lease.state(), kind).await
    }
    pub(super) async fn retained_media_response(
        &self,
        record: &super::media_obligations::MediaObligation,
        kind: MediaContinuationKind,
    ) -> Result<Option<MediaResponseState>, ExecutionError> {
        let supplement = record
            .supplements
            .iter()
            .find(|s| s.artifact.kind == kind)
            .ok_or(ExecutionError::Invalid)?;
        let receipt = supplement
            .admission
            .as_ref()
            .ok_or(ExecutionError::Invalid)?;
        let id = storage::identifier(&(KIND, &record.id, &receipt.response_id, kind.as_str()));
        let Some((mut state, version)) = self.0.read::<MediaResponseState>(&id).await? else {
            return Ok(None);
        };
        if state.kind != KIND
            || state.id != id
            || state.obligation != record.id
            || state.response_id != receipt.response_id
            || state.artifact != kind
            || state.text_sha256.len() != 64
            || state.text_len > MAX_TEXT
            || state.text_chunks.len() > 5
            || !matches!(
                state.status.as_str(),
                "completed" | "failed" | "incomplete" | "cancelled"
            )
        {
            return Err(ExecutionError::Corrupt);
        }
        state.version = version;
        Ok(Some(state))
    }
    pub(super) async fn freeze_media_response(
        &self,
        lease: &MediaLease,
        kind: MediaContinuationKind,
        status: &str,
        text: &str,
        custom: Option<CustomCheckpoint>,
    ) -> Result<MediaResponseState, ExecutionError> {
        self.guard_effect(lease).await?;
        if text.len() > MAX_TEXT
            || !matches!(status, "completed" | "failed" | "incomplete" | "cancelled")
        {
            return Err(ExecutionError::Invalid);
        }
        if let Some(existing) = self.media_response(lease, kind).await? {
            return if existing.status == status && existing.text_sha256 == sha256(text.as_bytes()) {
                Ok(existing)
            } else {
                Err(ExecutionError::IdempotencyConflict)
            };
        }
        let receipt = lease
            .state()
            .supplements
            .iter()
            .find(|s| s.artifact.kind == kind)
            .and_then(|s| s.admission.as_ref())
            .ok_or(ExecutionError::Invalid)?;
        let id =
            storage::identifier(&(KIND, &lease.record.id, &receipt.response_id, kind.as_str()));
        let digest = sha256(text.as_bytes());
        let mut chunks = Vec::new();
        let mut offset = 0;
        while offset < text.len() {
            self.guard_effect(lease).await?;
            let end = text.floor_char_boundary((offset + CHUNK).min(text.len()));
            let index = chunks.len();
            let chunk_id = storage::identifier(&(KIND, &id, &digest, index));
            let chunk = TextChunk {
                kind: KIND.into(),
                response: id.clone(),
                index,
                digest: digest.clone(),
                text: text[offset..end].into(),
            };
            self.0
                .write(
                    &chunk_id,
                    &lease.record.owner,
                    lease.record.retained_until,
                    &chunk,
                    None,
                )
                .await?;
            chunks.push(chunk_id);
            offset = end;
        }
        let mut state = MediaResponseState {
            kind: KIND.into(),
            id,
            obligation: lease.record.id.clone(),
            response_id: receipt.response_id.clone(),
            artifact: kind,
            status: status.into(),
            text_sha256: digest,
            text_len: text.len(),
            text_chunks: chunks,
            custom,
            native: NativeOutcome::NotStarted,
            version: 0,
        };
        self.guard_effect(lease).await?;
        state.version = self
            .0
            .write(
                &state.id,
                &lease.record.owner,
                lease.record.retained_until,
                &state,
                None,
            )
            .await?;
        Ok(state)
    }
    pub(super) async fn media_response_text(
        &self,
        lease: &MediaLease,
        state: &MediaResponseState,
    ) -> Result<String, ExecutionError> {
        self.guard(lease).await?;
        let mut text = String::new();
        for (index, id) in state.text_chunks.iter().enumerate() {
            if *id != storage::identifier(&(KIND, &state.id, &state.text_sha256, index)) {
                return Err(ExecutionError::Corrupt);
            }
            let (chunk, version) = self
                .0
                .read::<TextChunk>(id)
                .await?
                .ok_or(ExecutionError::Corrupt)?;
            if version != 1
                || chunk.kind != KIND
                || chunk.response != state.id
                || chunk.index != index
                || chunk.digest != state.text_sha256
                || chunk.text.len() > CHUNK
                || text.len() + chunk.text.len() > MAX_TEXT
            {
                return Err(ExecutionError::Corrupt);
            }
            text.push_str(&chunk.text);
        }
        if text.len() != state.text_len || sha256(text.as_bytes()) != state.text_sha256 {
            return Err(ExecutionError::Corrupt);
        }
        Ok(text)
    }
    pub(super) async fn record_media_response(
        &self,
        lease: &MediaLease,
        state: &mut MediaResponseState,
    ) -> Result<(), ExecutionError> {
        self.guard(lease).await?;
        state.version = self
            .0
            .write(
                &state.id,
                &lease.record.owner,
                lease.record.retained_until,
                state,
                Some(state.version),
            )
            .await?;
        Ok(())
    }
    pub(super) async fn settle_media_response(
        &self,
        lease: &mut MediaLease,
        kind: MediaContinuationKind,
    ) -> Result<(), ExecutionError> {
        let mut next = lease.record.clone();
        let item = next
            .supplements
            .iter_mut()
            .find(|s| s.artifact.kind == kind)
            .ok_or(ExecutionError::Invalid)?;
        if item.admission.is_none() {
            return Err(ExecutionError::Invalid);
        }
        item.settled = true;
        self.commit(lease, next).await
    }
}
