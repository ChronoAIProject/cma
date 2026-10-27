use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NativeStickerCapabilities {
    pub width: u32,
    pub height: u32,
    pub max_static_bytes: usize,
    pub max_animated_bytes: usize,
    pub max_frames: usize,
    pub max_duration_ms: u32,
    pub min_frame_duration_ms: u32,
}

impl NativeStickerCapabilities {
    pub(super) fn whatsapp() -> Self {
        Self {
            width: crate::whatsapp::outbound::sticker::SIDE,
            height: crate::whatsapp::outbound::sticker::SIDE,
            max_static_bytes: crate::whatsapp::outbound::sticker::MAX_STATIC_BYTES,
            max_animated_bytes: crate::whatsapp::outbound::sticker::MAX_ANIMATED_BYTES,
            max_frames: crate::whatsapp::outbound::sticker::MAX_FRAMES,
            max_duration_ms: crate::whatsapp::outbound::sticker::MAX_DURATION_MS,
            min_frame_duration_ms: crate::whatsapp::outbound::sticker::MIN_FRAME_DURATION_MS,
        }
    }
}
