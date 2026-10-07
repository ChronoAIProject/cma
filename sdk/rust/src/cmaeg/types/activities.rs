//! Activities wire types for the independent CMAEG v1 protocol.
use super::{
    AcceptedStatus, ActorKind, AttachmentKind, BindingId, ChannelId, ConversationId,
    ConversationKind, EventId, EventRef, MessageRef, Platform, ProblemCode,
};
use serde::{Deserialize, Serialize};

/// A capability to reply later to the original authenticated event destination.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyTargetResponse {
    /// Opaque bounded authority; do not log it.
    pub reply_target_ref: super::ReplyTargetRef,
    /// Expiry as Unix seconds, at most 24 hours after issuance.
    pub expires_at: i64,
}

/// CMAEG `Activity` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    /// Wire field `actor`.
    pub actor: Actor,
    /// Wire field `conversation`.
    pub conversation: ActivityConversation,
    /// Wire field `event_id`.
    pub event_id: EventId,
    /// Wire field `kind`.
    pub kind: ActivityKind,
    /// Wire field `occurred_at`.
    pub occurred_at: String,
    /// Wire field `schema_version`.
    pub schema_version: u64,
    /// Wire field `source`.
    pub source: SourceKind,
}

/// CMAEG `ActivityConversation` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityConversation {
    /// Wire field `id`.
    pub id: String,
    /// Wire field `kind`.
    pub kind: ConversationKind,
    /// Wire field `thread_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// CMAEG `ActivityKind` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ActivityKind {
    /// Wire variant `message`.
    #[serde(rename = "message")]
    Message {
        /// Shared by every message of one platform media group.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        album_id: Option<String>,
        /// Wire field `attachments`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<Attachment>>,
        /// Wire field `mentions_bot`.
        mentions_bot: bool,
        /// Wire field `reply_to`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<MessageRef>,
        /// Wire field `text`.
        text: String,
    },
    /// Wire variant `interaction`.
    #[serde(rename = "interaction")]
    Interaction {
        /// Wire field `action_id`.
        action_id: String,
        /// Wire field `block_id`.
        block_id: String,
        /// Wire field `values`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    },
    /// Wire variant `command`.
    #[serde(rename = "command")]
    Command {
        /// Wire field `args`.
        args: String,
        /// Wire field `name`.
        name: String,
    },
    /// Wire variant `event`.
    #[serde(rename = "event")]
    Event {
        /// Wire field `attachments`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<Attachment>>,
        /// Wire field `ce`.
        ce: CloudEvent,
    },
}

/// CMAEG `Actor` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Actor {
    /// Wire field `display_name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Wire field `id`.
    pub id: String,
    /// Wire field `kind`.
    pub kind: ActorKind,
}

/// CMAEG `Attachment` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    /// Wire field `duration_seconds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    /// Wire field `emoji`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// Wire field `filename`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Wire field `height`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u64>,
    /// Wire field `kind`.
    pub kind: AttachmentKind,
    /// Wire field `mime_type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Wire field `reference`.
    pub reference: String,
    /// Wire field `size_bytes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Where the bytes can be fetched; absent means not retrievable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<AttachmentSource>,
    /// Wire field `thumbnail`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<AttachmentSource>,
    /// Wire field `variant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<MediaVariant>,
    /// Wire field `width`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u64>,
}

/// CMAEG `AttachmentSource` wire shape: a non-secret locator bound into the
/// activity digest. Fetching still requires the event reference's authority.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AttachmentSource {
    /// Wire variant `nyxid_relay`.
    #[serde(rename = "nyxid_relay")]
    NyxidRelay {
        /// Wire field `index`.
        index: u64,
    },
    /// Wire variant `telegram`.
    #[serde(rename = "telegram")]
    Telegram {
        /// Wire field `file_id`.
        file_id: String,
        /// Wire field `file_unique_id`.
        file_unique_id: String,
    },
    /// Wire variant `url`.
    #[serde(rename = "url")]
    Url {
        /// Wire field `url`.
        url: String,
    },
    /// Wire variant `inline`.
    #[serde(rename = "inline")]
    Inline {
        /// Wire field `pointer`.
        pointer: String,
    },
}

/// CMAEG `MediaVariant` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaVariant {
    /// Wire value `photo`.
    #[serde(rename = "photo")]
    Photo,
    /// Wire value `document`.
    #[serde(rename = "document")]
    Document,
    /// Wire value `audio`.
    #[serde(rename = "audio")]
    Audio,
    /// Wire value `voice`.
    #[serde(rename = "voice")]
    Voice,
    /// Wire value `video`.
    #[serde(rename = "video")]
    Video,
    /// Wire value `video_note`.
    #[serde(rename = "video_note")]
    VideoNote,
    /// Wire value `animation`.
    #[serde(rename = "animation")]
    Animation,
    /// Wire value `sticker`.
    #[serde(rename = "sticker")]
    Sticker,
    /// Wire value `image`.
    #[serde(rename = "image")]
    Image,
    /// Wire value `file`.
    #[serde(rename = "file")]
    File,
    /// Wire value `media`.
    #[serde(rename = "media")]
    Media,
    /// Wire value `embedded`.
    #[serde(rename = "embedded")]
    Embedded,
}

/// CMAEG `Capabilities` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Wire field `attachments`.
    pub attachments: String,
    /// Wire field `buttons`.
    pub buttons: String,
    /// Wire field `cards`.
    pub cards: String,
    /// Wire field `edit`.
    pub edit: bool,
    /// Wire field `max_text_chars`.
    pub max_text_chars: u64,
    /// Wire field `typing`.
    pub typing: bool,
}

/// CMAEG `CloudEvent` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudEvent {
    /// Wire field `data`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Wire field `datacontenttype`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datacontenttype: Option<String>,
    /// Wire field `dataschema`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataschema: Option<String>,
    /// Wire field `id`.
    pub id: String,
    /// Wire field `source`.
    pub source: String,
    /// Wire field `specversion`.
    pub specversion: String,
    /// Wire field `subject`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Wire field `time`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    /// Wire field `type`.
    pub r#type: String,
    /// CloudEvents extension attributes.
    #[serde(flatten)]
    pub extensions: std::collections::BTreeMap<String, serde_json::Value>,
}

/// CMAEG `ContextResponse` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextResponse {
    /// Wire field `activity`.
    pub activity: PublicActivity,
    /// Wire field `activity_verified`.
    pub activity_verified: bool,
    /// Wire field `capabilities`.
    pub capabilities: Capabilities,
    /// Wire field `channel_id`.
    pub channel_id: ChannelId,
    /// Wire field `conversation_id`.
    pub conversation_id: ConversationId,
    /// Wire field `event_id`.
    pub event_id: EventId,
    /// Sealed event expiry as Unix seconds.
    pub expires_at: i64,
    /// Wire field `provider_binding_id`.
    pub provider_binding_id: BindingId,
    /// Wire field `source`.
    pub source: SourceKind,
}

/// CMAEG `EventAccepted` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct EventAccepted {
    /// Wire field `conversation_id`.
    pub conversation_id: ConversationId,
    /// Wire field `event_id`.
    pub event_id: EventId,
    /// Wire field `status`.
    pub status: AcceptedStatus,
}

/// CMAEG `EventContextSnapshot` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct EventContextSnapshot {
    /// Wire field `activity`.
    pub activity: Activity,
    /// Wire field `channel_id`.
    pub channel_id: ChannelId,
    /// Wire field `conversation_id`.
    pub conversation_id: ConversationId,
    /// Wire field `event_ref`.
    pub event_ref: EventRef,
    /// Wire field `schema_version`.
    pub schema_version: u64,
}

/// CMAEG `InteractionResult` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractionResult {
    /// Wire field `action_id`.
    pub action_id: String,
    /// Wire field `block_id`.
    pub block_id: String,
    /// Wire field `values`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

/// CMAEG `Problem` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    /// Wire field `code`.
    pub code: ProblemCode,
    /// Wire field `detail`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Wire field `instance`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// Wire field `status`.
    pub status: u64,
    /// Wire field `title`.
    pub title: String,
    /// Wire field `type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
}

/// CMAEG `PublicActivity` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicActivity {
    /// Wire field `actor`.
    pub actor: Actor,
    /// Wire field `conversation`.
    pub conversation: ActivityConversation,
    /// Wire field `kind`.
    pub kind: ActivityKind,
    /// Wire field `occurred_at`.
    pub occurred_at: String,
}

/// CMAEG `SourceKind` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SourceKind {
    /// Native Telegram ingress.
    #[serde(rename = "telegram")]
    Telegram {
        /// Source platform.
        platform: Platform,
        /// Telegram bot identifier.
        bot_id: String,
    },
    /// Wire variant `nyxid_relay`.
    #[serde(rename = "nyxid_relay")]
    NyxidRelay {
        /// Wire field `platform`.
        platform: Platform,
        /// Wire field `route_id`.
        route_id: String,
    },
    /// Wire variant `custom`.
    #[serde(rename = "custom")]
    Custom {},
}
