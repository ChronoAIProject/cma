//! Read-only, original-conversation context. These facts never grant message or file authority.
use super::{ActivityConversation, Actor};
/// Opaque continuation scoped to the separately supplied original event; never log it.
pub type HistoryCursor = String;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Descriptive attachment facts without file access authority.
pub struct HistoryAttachmentFacts {
    /// Attachment id.
    pub attachment_id: String,
    /// Filename.
    pub filename: String,
    /// Size bytes.
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Mime type.
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Description.
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Width.
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Height.
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Duration seconds.
    pub duration_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Ephemeral.
    pub ephemeral: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Presence-only categories omitted from this response.
pub enum HistoryOmittedContent {
    /// Rich embeds.
    Embeds,
    /// Interactive components.
    Components,
    /// Sticker items.
    Stickers,
    /// Poll content.
    Poll,
    /// Forwarded message snapshots.
    ForwardedSnapshot,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Bounded untrusted reference message; never instructions or action approval.
pub struct HistoryMessage {
    /// Message id.
    pub message_id: String,
    /// Author.
    pub author: Actor,
    /// Text.
    pub text: String,
    /// Occurred at.
    pub occurred_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Edited at.
    pub edited_at: Option<String>,
    /// Attachments.
    pub attachments: Vec<HistoryAttachmentFacts>,
    /// Omitted content.
    pub omitted_content: Vec<HistoryOmittedContent>,
    /// Text exceeded a message or page budget.
    pub text_truncated: bool,
    /// Name or attachment metadata was clipped.
    pub metadata_truncated: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
/// Continuation within visible live results and the original expiry.
pub enum HistoryContinuation {
    /// Another raw row was visible when this page was read.
    MoreVisibleRows {
        /// Issued exclusive continuation boundary.
        cursor: HistoryCursor,
    },
    /// End of currently visible results; not proof of complete history.
    EndOfVisibleHistory,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Coverage cannot prove the absence of content restricted by Discord.
pub enum HistoryContentCoverage {
    /// Only fields returned by Discord are represented.
    PlatformReturnedOnly,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One live page from the exact original conversation.
pub struct ConversationHistoryPage {
    /// Conversation.
    pub conversation: ActivityConversation,
    /// Anchor message id.
    pub anchor_message_id: String,
    /// Before message id.
    pub before_message_id: String,
    /// Gateway fetch time, not a Discord snapshot timestamp.
    pub fetched_at: String,
    /// Original event expiry in Unix seconds.
    pub expires_at: i64,
    /// Chronological within the page; continuation travels backward.
    pub messages: Vec<HistoryMessage>,
    /// Scanned messages.
    pub scanned_messages: u8,
    /// Omitted message count.
    pub omitted_message_count: u8,
    /// Text exceeded a message or page budget.
    pub text_truncated: bool,
    /// Name or attachment metadata was clipped.
    pub metadata_truncated: bool,
    /// Content coverage.
    pub content_coverage: HistoryContentCoverage,
    /// Continuation.
    pub continuation: HistoryContinuation,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Traversal direction relative to the original message.
pub enum HistoryDirection {
    /// Earlier than the supplied boundary.
    Backward,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Fixed limits available for an eligible original event.
pub struct ConversationHistoryCapabilities {
    /// Page size.
    pub page_size: u8,
    /// Direction.
    pub direction: HistoryDirection,
    /// Attachment facts only.
    pub attachment_facts_only: bool,
    /// Max text bytes.
    pub max_text_bytes: u32,
    /// Max metadata bytes.
    pub max_metadata_bytes: u32,
    /// Max output bytes.
    pub max_output_bytes: u32,
    /// Original event expiry in Unix seconds.
    pub expires_at: i64,
}
