//! Messages wire types for the independent CMAEG v1 protocol.
use super::{AttachmentKind, MessageRef, OptionStyle, RenderingOutcome, TypingOutcome};
use serde::{Deserialize, Serialize};

/// CMAEG `Block` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Block {
    /// Wire variant `text`.
    #[serde(rename = "text")]
    Text {
        /// Wire field `text`.
        text: String,
    },
    /// Wire variant `card`.
    #[serde(rename = "card")]
    Card {
        /// Wire field `body`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        /// Wire field `fields`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<Field>>,
        /// Wire field `image`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<String>,
        /// Wire field `title`.
        title: String,
    },
    /// Wire variant `buttons`.
    #[serde(rename = "buttons")]
    Buttons {
        /// Wire field `id`.
        id: String,
        /// Wire field `options`.
        options: Vec<ChoiceOption>,
    },
    /// Wire variant `choice`.
    #[serde(rename = "choice")]
    Choice {
        /// Wire field `id`.
        id: String,
        /// Wire field `multi`.
        multi: bool,
        /// Wire field `options`.
        options: Vec<ChoiceOption>,
        /// Wire field `prompt`.
        prompt: String,
    },
    /// Wire variant `form`.
    #[serde(rename = "form")]
    Form {
        /// Wire field `fields`.
        fields: Vec<FormField>,
        /// Wire field `id`.
        id: String,
    },
    /// Wire variant `attachment`.
    #[serde(rename = "attachment")]
    Attachment {
        /// Wire field `caption`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        /// Wire field `kind`.
        kind: AttachmentKind,
        /// Wire field `reference`.
        reference: String,
    },
    /// Wire variant `link`.
    #[serde(rename = "link")]
    Link {
        /// Wire field `label`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// Wire field `url`.
        url: String,
    },
}

/// CMAEG `BlockRendering` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockRendering {
    /// Wire field `index`.
    pub index: u64,
    /// Wire field `outcome`.
    pub outcome: BlockRenderingOutcome,
}

/// CMAEG `BlockRenderingOutcome` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BlockRenderingOutcome {
    /// Wire variant `exact`.
    #[serde(rename = "exact")]
    Exact {},
    /// Wire variant `degraded`.
    #[serde(rename = "degraded")]
    Degraded {
        /// Wire field `how`.
        how: String,
    },
    /// Wire variant `dropped`.
    #[serde(rename = "dropped")]
    Dropped {
        /// Wire field `why`.
        why: String,
    },
}

/// CMAEG `ChoiceOption` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOption {
    /// Wire field `id`.
    pub id: String,
    /// Wire field `label`.
    pub label: String,
    /// Wire field `selected`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    /// Wire field `style`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<OptionStyle>,
}

/// CMAEG `DeliveryOutcome` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome")]
pub enum DeliveryOutcome {
    /// Wire variant `confirmed`.
    #[serde(rename = "confirmed")]
    Confirmed {
        /// Wire field `platform_message_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        platform_message_id: Option<String>,
    },
    /// Wire variant `rejected`.
    #[serde(rename = "rejected")]
    Rejected {
        /// Wire field `reason`.
        reason: String,
    },
    /// Wire variant `rate_limited`.
    #[serde(rename = "rate_limited")]
    RateLimited {
        /// Wire field `retry_after`.
        retry_after: u64,
    },
    /// Wire variant `unknown`.
    #[serde(rename = "unknown")]
    Unknown {
        /// Wire field `reason`.
        reason: String,
    },
    /// Wire variant `unsupported`.
    #[serde(rename = "unsupported")]
    Unsupported {},
}

/// CMAEG `Field` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    /// Wire field `label`.
    pub label: String,
    /// Wire field `value`.
    pub value: String,
}

/// CMAEG `FormField` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct FormField {
    /// Wire field `id`.
    pub id: String,
    /// Wire field `kind`.
    pub kind: FormFieldKind,
    /// Wire field `label`.
    pub label: String,
    /// Wire field `required`.
    pub required: bool,
}

/// CMAEG `FormFieldKind` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FormFieldKind {
    /// Wire variant `text`.
    #[serde(rename = "text")]
    Text {},
    /// Wire variant `number`.
    #[serde(rename = "number")]
    Number {},
    /// Wire variant `choice`.
    #[serde(rename = "choice")]
    Choice {
        /// Wire field `options`.
        options: Vec<ChoiceOption>,
    },
    /// Wire variant `date`.
    #[serde(rename = "date")]
    Date {},
}

/// One confirmed native text part; the aggregate outcome can still be unknown.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfirmedMessagePart {
    /// Zero-based rendered text position.
    pub index: u16,
    /// Exact native destination under the original consumer authority.
    pub message_ref: MessageRef,
}

/// CMAEG `IntentReceipt` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentReceipt {
    /// Confirmed split-text parts. Legacy receipts omit this field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_parts: Vec<ConfirmedMessagePart>,
    /// Wire field `delivery`.
    pub delivery: DeliveryOutcome,
    /// Wire field `message_ref`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<MessageRef>,
    /// Wire field `rendering`.
    pub rendering: RenderingReport,
}

/// CMAEG `OutboundMessage` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboundMessage {
    /// Resend only the exact target event's retained native Lark sticker.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub reply_with_received_sticker: Option<bool>,
    /// Update-only explicit removal; supported only when clear_media is advertised.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub clear_native_attachments: Option<bool>,
    /// Update-only patch: omitted preserves; an empty caption/text clears; null refuses.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub native_media_update: Option<super::NativeMediaUpdate>,
    /// Approved WhatsApp template, exclusive of other message content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whatsapp_template: Option<super::WhatsappTemplate>,
    /// Explicit native media; omitted preserves link-only attachment blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_attachments: Option<Vec<NativeAttachment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Explicit native-thread choice; absence preserves legacy delivery.
    pub thread_reply: Option<bool>,
    /// Wire field `blocks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<Block>>,
    /// Wire field `reply_to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<MessageRef>,
    /// Wire field `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// CMAEG `RenderingReport` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderingReport {
    /// Wire field `blocks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<BlockRendering>>,
    /// Wire field `outcome`.
    pub outcome: RenderingOutcome,
}

/// CMAEG `TypingReceipt` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct TypingReceipt {
    /// Wire field `outcome`.
    pub outcome: TypingOutcome,
}

/// Native media category supported by the destination relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeMediaKind {
    /// Image rendered by the destination platform.
    Image,
    /// Downloadable file.
    File,
    /// Audio attachment.
    Audio,
    /// Video attachment.
    Video,
    /// Sticker presentation, only when advertised by the event destination.
    Sticker,
}

/// Public HTTPS URL or a bounded standard-base64 body, never a local file path.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeMediaSource {
    /// Public HTTPS resource fetched by the relay.
    Url {
        /// HTTPS URL without credentials or a fragment.
        url: String,
    },
    /// Inline bytes using standard padded base64.
    Base64 {
        /// Encoded attachment bytes, subject to the aggregate decoded limit.
        data: String,
    },
}

/// Explicit native attachment; consult context.native_media before sending.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeAttachment {
    /// Destination media category.
    pub kind: NativeMediaKind,
    /// Public URL or inline attachment bytes.
    pub source: NativeMediaSource,
    /// Optional filename without directory components.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Optional MIME type without parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Optional caption, bounded by the gateway contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

/// Deployment-attested outbound media capabilities.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMediaCapabilities {
    /// Ordered album constraints; omitted when disabled for this destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub albums: Option<NativeAlbumCapabilities>,
    /// Same-message native upload and controls, omitted when not supported for this event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controls: Option<MediaControlsCapabilities>,
    /// Media categories supported for this event's destination.
    pub kinds: Vec<String>,
    /// Accepted source forms, such as url or base64.
    pub sources: Vec<String>,
    /// Maximum attachment count per logical reply.
    pub max_attachments: u64,
    /// Maximum aggregate decoded inline bytes per logical reply.
    pub max_inline_bytes: u64,
    /// Exact MIME subset, when declared by the destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_types: Option<Vec<String>>,
    /// Aggregate decoded/downloaded bytes in one reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_bytes: Option<u64>,
    /// Per-image encoded byte ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_image_bytes: Option<u64>,
    /// Caption characters; shared metadata byte limits still apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_caption_chars: Option<u32>,
    /// Structural sticker limits, absent on unsupported destinations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticker: Option<NativeStickerCapabilities>,
    /// Bounded video profile; absent when no destination-specific profile is declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<NativeVideoCapabilities>,
}

/// Exact album constraints; no public operation authority is issued for its members.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeAlbumCapabilities {
    /// Minimum number of attachments sent together.
    pub min_attachments: u64,
    /// Maximum number of attachments sent together.
    pub max_attachments: u64,
    /// An album's kinds must all belong to one group.
    pub kind_groups: Vec<Vec<String>>,
    /// Whether interactive controls can accompany an album.
    pub controls: bool,
    /// Whether album delivery issues message operation references.
    pub message_operations: bool,
}

/// Native upload and controls in one confirmed platform message.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaControlsCapabilities {
    /// Exact kinds allowed with controls; absent means the parent media kinds apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    /// Maximum upload count in the same controlled message.
    pub max_attachments: u64,
    /// Whether native buttons can accompany the upload.
    pub buttons: bool,
    /// Whether native choices can accompany the upload.
    pub choices: bool,
    /// Caption character limit; metadata byte bounds also apply.
    pub caption_max_chars: u32,
    /// Whether content/control edits may preserve this message's confirmed upload.
    pub update: bool,
    /// Replace the supported upload set on the same durable card through a confirmed PATCH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_media: Option<bool>,
    /// Whether explicit clear_native_attachments may remove every upload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_media: Option<bool>,
}

/// Exact dimensions, size and animation limits for native sticker delivery.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeStickerCapabilities {
    /// Required canvas width.
    pub width: u32,
    /// Required canvas height.
    pub height: u32,
    /// Encoded static sticker byte ceiling.
    pub max_static_bytes: u64,
    /// Encoded animated sticker byte ceiling.
    pub max_animated_bytes: u64,
    /// Frame count ceiling for one animation cycle.
    pub max_frames: u64,
    /// Duration ceiling for one animation cycle, in milliseconds.
    pub max_duration_ms: u32,
    /// Minimum duration per animation frame, in milliseconds.
    pub min_frame_duration_ms: u32,
}

/// Structural native video limits; not a full decoding or client-acceptance guarantee.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeVideoCapabilities {
    /// Video codec, currently h264.
    pub video_codec: String,
    /// Codec profile, currently baseline.
    pub video_profile: String,
    /// Optional audio track's codec, currently aac_lc.
    pub audio_codec: String,
    /// Maximum audio track count; a silent video is allowed.
    pub max_audio_tracks: u32,
    /// Maximum display width in pixels.
    pub max_width: u32,
    /// Maximum display height in pixels.
    pub max_height: u32,
    /// Maximum frames per second.
    pub max_frame_rate: u32,
    /// Maximum track duration in milliseconds.
    pub max_duration_ms: u32,
    /// Total samples across video and optional audio.
    pub max_samples: u64,
    /// Whether interlaced video is refused.
    pub progressive_only: bool,
    /// Maximum coded slices per frame.
    pub max_slices_per_frame: u32,
    /// Whether the movie metadata must precede media bytes.
    pub fast_start_required: bool,
}
