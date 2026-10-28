//! Read-only, original-conversation context. These facts never grant message or file authority.
use crate::{
    activity::{ActivityConversation, Actor},
    seal::HistoryCursor,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const HISTORY_PAGE_SIZE: u8 = 25;
pub const HISTORY_TEXT_BYTES: usize = 32 * 1024;
pub const HISTORY_METADATA_BYTES: usize = 16 * 1024;
pub const HISTORY_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryAttachmentFacts {
    pub attachment_id: String,
    pub filename: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryOmittedContent {
    Embeds,
    Components,
    Stickers,
    Poll,
    ForwardedSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryMessage {
    pub message_id: String,
    pub author: Actor,
    pub text: String,
    pub occurred_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    pub attachments: Vec<HistoryAttachmentFacts>,
    pub omitted_content: Vec<HistoryOmittedContent>,
    pub text_truncated: bool,
    pub metadata_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryContinuation {
    /// More provider rows remain to scan; native thread pages may contain no earlier messages.
    MoreVisibleRows {
        cursor: HistoryCursor,
    },
    EndOfVisibleHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryContentCoverage {
    PlatformReturnedOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationHistoryPage {
    pub conversation: ActivityConversation,
    pub anchor_message_id: String,
    /// Original/last scanned pre-cutoff ID. Native provider scan position remains in the cursor.
    pub before_message_id: String,
    /// Gateway fetch time, not a platform snapshot timestamp.
    pub fetched_at: String,
    pub expires_at: i64,
    /// Chronological within the page; continuation travels backward.
    pub messages: Vec<HistoryMessage>,
    /// Raw rows scanned, including native thread rows at/after the original timestamp.
    pub scanned_messages: u8,
    /// Scanned rows omitted by cutoff, deletion, unsupported content or actor policy.
    pub omitted_message_count: u8,
    pub text_truncated: bool,
    pub metadata_truncated: bool,
    pub content_coverage: HistoryContentCoverage,
    pub continuation: HistoryContinuation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryDirection {
    Backward,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationHistoryCapabilities {
    pub page_size: u8,
    pub direction: HistoryDirection,
    pub attachment_facts_only: bool,
    pub max_text_bytes: u32,
    pub max_metadata_bytes: u32,
    pub max_output_bytes: u32,
    pub expires_at: i64,
}
