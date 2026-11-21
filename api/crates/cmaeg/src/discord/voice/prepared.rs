//! Encoded bytes and derived metadata stay together until the single send.
use super::{metadata, pcm_metadata, Metadata, MAX_BYTES, MAX_SECONDS};
use crate::media::{
    budget::{InflightBytes, InflightPermit},
    outbound::{PreparedFile, MAX_OUTBOUND_BYTES},
    outbound_profiles::ogg,
    transform::{pcm, AudioRequest, MediaTransformer},
};
use bytes::Bytes;
use cmaeg_core::ids::ChannelId;
use std::{sync::Arc, time::Duration};
use tokio::time::{timeout_at, Instant};

/// Clones share the inbound transformer's process slot, watchdog and byte budget.
#[derive(Clone)]
pub(crate) struct Decoder {
    transformer: Arc<dyn MediaTransformer>,
    inflight: InflightBytes,
    max_inflight: u64,
}
impl Decoder {
    pub(crate) fn new(
        transformer: Arc<dyn MediaTransformer>,
        inflight: InflightBytes,
        max_inflight: u64,
    ) -> Self {
        Self {
            transformer,
            inflight,
            max_inflight,
        }
    }
}

/// No public constructor or mutable fields: the upload cannot change after proof.
pub(crate) struct PreparedVoice {
    file: PreparedFile,
    metadata: Metadata,
    _encoded_budget: Option<InflightPermit>,
}
impl PreparedVoice {
    pub(crate) fn file(&self) -> &PreparedFile {
        &self.file
    }
    pub(crate) fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

pub(crate) async fn prepare(
    mut file: PreparedFile,
    mime_omitted: bool,
    filename_omitted: bool,
    channel: &ChannelId,
    decoder: Option<&Decoder>,
) -> Option<PreparedVoice> {
    let is_ogg = file.bytes.starts_with(b"OggS");
    if mime_omitted {
        file.mime = if is_ogg { "audio/ogg" } else { "audio/wav" }.into();
    }
    if filename_omitted {
        file.filename = if is_ogg { "voice.ogg" } else { "voice.wav" }.into();
    }
    if file.mime == "audio/wav" {
        let metadata = metadata(&file)?;
        return Some(PreparedVoice {
            file,
            metadata,
            _encoded_budget: None,
        });
    }
    if file.mime != "audio/ogg"
        || file.caption.is_some()
        || file.bytes.len() as u64 > MAX_OUTBOUND_BYTES
        || !ogg::mono_opus(&file.bytes)
    {
        return None;
    }
    let decoder = decoder?;
    // Retain the exact upload plus the transformer's input copy. Leave room for
    // its separately reserved PCM stdout so a small configured budget fails now.
    let held_bytes = 2 * file.bytes.len() as u64;
    if held_bytes + MAX_BYTES as u64 + 2048 > decoder.max_inflight {
        return None;
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    let held = decoder.inflight.acquire(held_bytes, deadline).await?;
    let pcm = timeout_at(
        deadline,
        decoder.transformer.audio(AudioRequest {
            channel: channel.clone(),
            input: Bytes::copy_from_slice(&file.bytes),
            seconds: MAX_SECONDS as u16,
        }),
    )
    .await
    .ok()?
    .ok()?;
    // The helper intentionally stops at 120 seconds. A result at that cap cannot
    // prove the whole input was decoded, so never silently send a clipped preview.
    if pcm.truncated
        || pcm.data_len >= MAX_SECONDS * pcm::PCM_BYTES_PER_SECOND
        || pcm.data_len != pcm.wav.len().checked_sub(pcm::WAV_HEADER_BYTES)?
    {
        return None;
    }
    let metadata = pcm_metadata(&pcm.wav)?;
    Some(PreparedVoice {
        file,
        metadata,
        _encoded_budget: Some(held),
    })
}

#[cfg(test)]
mod tests;
