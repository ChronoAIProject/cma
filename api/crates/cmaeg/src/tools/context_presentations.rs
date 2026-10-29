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
}

impl NativePresentationProfile {
    pub(super) fn voice(caption: bool) -> Self {
        Self {
            kind: "voice".into(),
            mime_types: vec!["audio/ogg".into()],
            max_bytes: crate::media::outbound::MAX_OUTBOUND_BYTES,
            caption,
            profile: "One complete mono Opus Ogg stream; no transcoding".into(),
            message_operations: false,
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
        }, Self {
            kind: "sticker".into(),
            mime_types: vec!["image/webp".into()],
            max_bytes: crate::whatsapp::outbound::sticker::MAX_STATIC_BYTES as u64,
            caption: false,
            profile: "Static WebP, exactly 512x512; animated WebP, TGS and WebM are unsupported".into(),
            message_operations: false,
        }]
    }
}
