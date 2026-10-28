use super::{
    native_whatsapp_templates, Activity, EventRefPayload, NativeStickerCapabilities,
    NativeVideoCapabilities, ResolvedChannel, ToolsState,
};
use cmaeg_core::activity::SourceKind;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NativeMediaCapabilities {
    /// One upload and native controls in the same message; absent while issuance is disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controls: Option<MediaControlsCapabilities>,
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

/// Exact media/control combination supported for this event's verified destination.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MediaControlsCapabilities {
    /// Exact attachment kinds supported with controls; absent means the parent kinds apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    pub max_attachments: usize,
    pub buttons: bool,
    pub choices: bool,
    pub caption_max_chars: u32,
    pub update: bool,
    /// Replace one upload on the same durable card through a confirmed native PATCH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_media: Option<bool>,
}
impl MediaControlsCapabilities {
    pub(super) fn single(caption_max_chars: u32) -> Self {
        Self {
            kinds: None,
            max_attachments: 1,
            buttons: true,
            choices: true,
            caption_max_chars,
            update: false,
            replace_media: None,
        }
    }
}

impl NativeMediaCapabilities {
    pub(super) fn for_event(
        state: &ToolsState,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        activity: &Activity,
        verified: bool,
    ) -> Option<Self> {
        if matches!(
            event.reply,
            cmaeg_core::seal::ReplyAddress::WhatsappGroup(_)
        ) {
            Self::native_whatsapp_group(
                state.native_whatsapp_outbound_media_enabled
                    && state.native_whatsapp_groups_enabled
                    && native_whatsapp_templates(true, channel, event, activity, verified),
            )
        } else if matches!(activity.source, SourceKind::NativeLark(_)) {
            Self::native_lark(state.native_lark_outbound_media_enabled).map(|mut capabilities| {
                if verified
                    && state.native_lark_media_controls_enabled
                    && crate::interactions::native_lark::eligible(channel, event, activity)
                {
                    let mut controls = MediaControlsCapabilities::single(0);
                    controls.kinds = Some(vec!["image".into()]);
                    if state.native_lark_multi_image_controls_enabled {
                        controls.max_attachments = cmaeg_core::message::MAX_NATIVE_ATTACHMENTS;
                    }
                    controls.update = true;
                    capabilities.controls = Some(controls);
                }
                capabilities
            })
        } else if matches!(activity.source, SourceKind::Whatsapp { .. }) {
            Self::native_whatsapp(state.native_whatsapp_outbound_media_enabled).map(
                |mut capabilities| {
                    if verified
                        && state.native_whatsapp_media_controls_enabled
                        && state.native_whatsapp_interactions_enabled
                        && (event.consumer.is_none() || state.consumer_interactions_enabled)
                        && matches!(event.reply, cmaeg_core::seal::ReplyAddress::Whatsapp { .. })
                        && crate::interactions::whatsapp::eligible(channel, event, activity)
                    {
                        let mut controls = MediaControlsCapabilities::single(0);
                        controls.kinds = Some(["image", "file", "video"].map(String::from).into());
                        controls.choices = false;
                        capabilities.controls = Some(controls);
                    }
                    capabilities
                },
            )
        } else {
            Self::for_source(&activity.source, state.native_media_enabled).map(
                |mut capabilities| {
                    let discord_controls = state.native_discord_media_controls_enabled
                        && state.discord_interactions_enabled
                        && crate::interactions::discord::eligible(channel, event, activity);
                    let telegram_controls = state.native_telegram_media_controls_enabled
                        && crate::interactions::telegram_media::eligible(channel, event, activity);
                    if verified
                        && (event.consumer.is_none() || state.consumer_interactions_enabled)
                        && (discord_controls || telegram_controls)
                    {
                        let mut controls = MediaControlsCapabilities::single(1024);
                        controls.update = (discord_controls
                            && state.native_discord_media_card_edits_enabled)
                            || (telegram_controls
                                && state.native_telegram_media_control_edits_enabled);
                        controls.replace_media = (discord_controls
                            && state.native_discord_media_card_edits_enabled
                            && state.native_discord_media_card_replacements_enabled)
                            .then_some(true);
                        capabilities.controls = Some(controls);
                    }
                    capabilities
                },
            )
        }
    }

    pub(super) fn native_lark(enabled: bool) -> Option<Self> {
        enabled.then(|| Self {
            controls: None,
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
            controls: None,
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
    pub(super) fn native_whatsapp_group(enabled: bool) -> Option<Self> {
        Self::native_whatsapp(enabled).map(|mut capabilities| {
            capabilities.kinds.retain(|kind| kind != "sticker");
            if let Some(mimes) = &mut capabilities.mime_types {
                mimes.retain(|mime| mime != "image/webp");
            }
            capabilities.sticker = None;
            capabilities
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
            controls: None,
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
