//! Native voice metadata from actual canonical PCM samples, never declared timing.
use crate::media::{outbound::PreparedFile, transform::pcm};
use base64::{engine::general_purpose::STANDARD, Engine};
use bytes::BytesMut;
use cmaeg_core::message::{NativeMediaKind, OutboundMessage};

mod prepared;
pub(crate) use prepared::{prepare, Decoder, PreparedVoice};

pub(crate) const MAX_SECONDS: usize = 120;
pub(crate) const MAX_BYTES: usize = pcm::WAV_HEADER_BYTES + MAX_SECONDS * pcm::PCM_BYTES_PER_SECOND;
pub(crate) const FLAG: u64 = 1 << 13;

pub(crate) fn requested(message: &OutboundMessage) -> bool {
    message
        .native_attachments
        .iter()
        .any(|item| item.kind == NativeMediaKind::Voice)
}
pub(crate) fn eligible(message: &OutboundMessage) -> bool {
    message.validate().is_ok()
        && message.native_attachments.len() == 1
        && message.native_attachments[0].kind == NativeMediaKind::Voice
        && message.native_attachments[0].caption.is_none()
        && message.text.is_none()
        && message.blocks.is_empty()
        && message.reply_to.is_none()
        && message.whatsapp_template.is_none()
        && message.native_media_update.is_none()
        && !message.clear_native_attachments
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Metadata {
    pub duration_secs: f64,
    pub waveform: String,
}

/// Accept only the exact format produced by the existing sandbox PCM helper:
/// RIFF/WAVE, one fmt/data pair, mono signed 16-bit LE, 16kHz, no trailing bytes.
/// No compressed stream is decoded and no caller duration/waveform is accepted.
pub(crate) fn metadata(file: &PreparedFile) -> Option<Metadata> {
    if file.mime != "audio/wav" || file.caption.is_some() {
        return None;
    }
    pcm_metadata(&file.bytes)
}

pub(super) fn pcm_metadata(bytes: &[u8]) -> Option<Metadata> {
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let data = bytes.get(pcm::WAV_HEADER_BYTES..)?;
    // At least one 100ms bin; shorter clips cannot provide the declared preview.
    if data.len() < pcm::PCM_BYTES_PER_SECOND / 10 || !data.len().is_multiple_of(2) {
        return None;
    }
    let mut expected = BytesMut::zeroed(pcm::WAV_HEADER_BYTES);
    pcm::write_wav_header(&mut expected, u32::try_from(data.len()).ok()?);
    if bytes[..pcm::WAV_HEADER_BYTES] != expected[..] {
        return None;
    }
    let samples = data.len() / 2;
    // Partition the entire recording into <=256 contiguous bins. Each covers
    // >=100ms. A byte is the bin's peak absolute PCM amplitude scaled 0..255.
    // This is our documented preview convention, not a Discord codec contract.
    let points = (data.len() / (pcm::PCM_BYTES_PER_SECOND / 10)).min(256);
    let mut waveform = Vec::with_capacity(points);
    for index in 0..points {
        let start = index * samples / points;
        let end = (index + 1) * samples / points;
        let peak = data[start * 2..end * 2]
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs())
            .max()?;
        waveform.push(((u32::from(peak) * 255 + 16_384) / 32_768) as u8);
    }
    Some(Metadata {
        duration_secs: data.len() as f64 / pcm::PCM_BYTES_PER_SECOND as f64,
        waveform: STANDARD.encode(waveform),
    })
}

#[cfg(test)]
#[path = "voice_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) mod test_support;
