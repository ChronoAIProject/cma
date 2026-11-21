use serde::Serialize;
use utoipa::ToSchema;

/// Explicit native presentation profiles; ordinary audio/video keep their existing meaning.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NativePresentationProfile {
    pub kind: String,
    pub mime_types: Vec<String>,
    pub max_bytes: u64,
    pub caption: bool,
    pub profile: String,
    pub message_operations: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_attachments: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<bool>,
}

impl NativePresentationProfile {
    pub(super) fn discord_opus_voice() -> Self {
        Self {
            kind: "voice".into(), mime_types: vec!["audio/ogg".into()],
            max_bytes: crate::media::outbound::MAX_OUTBOUND_BYTES,
            caption: false, max_attachments: Some(1), text: Some(false),
            profile: "One complete mono Opus Ogg stream, 0.1 to less than 120 seconds; uploaded unchanged; duration and waveform derived with the shared sandboxed decoder; no text or blocks".into(),
            message_operations: false,
        }
    }
    pub(super) fn discord_voice() -> Self {
        Self {
            kind: "voice".into(), mime_types: vec!["audio/wav".into()],
            max_bytes: crate::discord::voice::MAX_BYTES as u64,
            caption: false, max_attachments: Some(1), text: Some(false),
            profile: "Canonical mono signed 16-bit little-endian PCM WAV at 16kHz, 0.1–120 seconds; one attachment, no text or blocks; complete waveform derived from samples".into(),
            message_operations: false,
        }
    }

    pub(super) fn voice(caption: bool) -> Self {
        Self {
            kind: "voice".into(),
            mime_types: vec!["audio/ogg".into()],
            max_bytes: crate::media::outbound::MAX_OUTBOUND_BYTES,
            caption,
            profile: "One complete mono Opus Ogg stream; no transcoding".into(),
            message_operations: false,
            max_attachments: None,
            text: None,
        }
    }
    pub(super) fn telegram() -> Vec<Self> {
        vec![Self::voice(true), Self {
            kind: "animation".into(),
            mime_types: vec!["image/gif".into(), "video/mp4".into()],
            max_bytes: crate::media::outbound::MAX_OUTBOUND_BYTES,
            caption: true,
            profile: "GIF: at most 1920x1080 and 256 frames; MP4: bounded Baseline H.264 without audio".into(),
            message_operations: false,
            max_attachments: None,
            text: None,
        }, Self {
            kind: "sticker".into(),
            mime_types: vec!["image/webp".into()],
            max_bytes: crate::whatsapp::outbound::sticker::MAX_STATIC_BYTES as u64,
            caption: false,
            profile: "Static WebP, exactly 512x512; animated WebP, TGS and WebM are unsupported".into(),
            message_operations: false,
            max_attachments: None,
            text: None,
        }]
    }
}
