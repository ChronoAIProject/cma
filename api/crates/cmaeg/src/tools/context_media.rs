use super::{NativeStickerCapabilities, NativeVideoCapabilities};
use cmaeg_core::activity::SourceKind;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NativeMediaCapabilities {
    pub kinds: Vec<String>,
    pub sources: Vec<String>,
    pub max_attachments: usize,
    pub max_inline_bytes: usize,
    /// Exact accepted MIME types when the transport supports a bounded subset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_types: Option<Vec<String>>,
    /// Aggregate decoded/downloaded bytes in one reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_bytes: Option<u64>,
    /// Per-image encoded byte ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_image_bytes: Option<u64>,
    /// Per-attachment caption character ceiling; shared metadata byte limits still apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_caption_chars: Option<u32>,
    /// Exact structural sticker limits; absent when sticker delivery is unsupported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticker: Option<NativeStickerCapabilities>,
    /// Exact structural video profile; absent when no bounded profile is declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<NativeVideoCapabilities>,
}

impl NativeMediaCapabilities {
    pub(super) fn native_lark(enabled: bool) -> Option<Self> {
        enabled.then(|| Self {
            kinds: ["image", "file", "audio", "video"].map(String::from).into(),
            sources: ["url", "base64"].map(String::from).into(),
            max_attachments: cmaeg_core::message::MAX_NATIVE_ATTACHMENTS,
            max_inline_bytes: cmaeg_core::message::MAX_NATIVE_INLINE_BYTES,
            mime_types: Some(
                crate::native_lark::outbound::MIME_TYPES
                    .map(String::from)
                    .into(),
            ),
            max_total_bytes: Some(crate::media::outbound::MAX_OUTBOUND_BYTES),
            max_image_bytes: Some(crate::media::outbound::MAX_OUTBOUND_BYTES),
            max_caption_chars: Some(0),
            sticker: None,
            video: Some(NativeVideoCapabilities::bounded_baseline()),
        })
    }

    pub(super) fn native_whatsapp(enabled: bool) -> Option<Self> {
        enabled.then(|| Self {
            kinds: ["image", "file", "audio", "sticker", "video"]
                .map(String::from)
                .into(),
            sources: ["url", "base64"].map(String::from).into(),
            max_attachments: cmaeg_core::message::MAX_NATIVE_ATTACHMENTS,
            max_inline_bytes: cmaeg_core::message::MAX_NATIVE_INLINE_BYTES,
            mime_types: Some(
                crate::whatsapp::outbound::MIME_TYPES
                    .map(String::from)
                    .into(),
            ),
            max_total_bytes: Some(crate::media::outbound::MAX_OUTBOUND_BYTES),
            max_image_bytes: Some(crate::whatsapp::outbound::MAX_IMAGE_BYTES as u64),
            max_caption_chars: Some(1024),
            sticker: Some(NativeStickerCapabilities::whatsapp()),
            video: Some(NativeVideoCapabilities::whatsapp()),
        })
    }
    pub(super) fn for_source(source: &SourceKind, enabled: bool) -> Option<Self> {
        let supported = match source {
            SourceKind::Discord { .. } | SourceKind::Telegram { .. } => true,
            SourceKind::NyxidRelay { platform, .. } => {
                enabled && crate::delivery::relay_client::native_media_supported(*platform)
            }
            _ => false,
        };
        if !supported {
            return None;
        }
        Some(Self {
            kinds: ["image", "file", "audio", "video"].map(String::from).into(),
            sources: ["url", "base64"].map(String::from).into(),
            max_attachments: cmaeg_core::message::MAX_NATIVE_ATTACHMENTS,
            max_inline_bytes: cmaeg_core::message::MAX_NATIVE_INLINE_BYTES,
            mime_types: None,
            max_total_bytes: None,
            max_image_bytes: None,
            max_caption_chars: None,
            sticker: None,
            video: None,
        })
    }
}
