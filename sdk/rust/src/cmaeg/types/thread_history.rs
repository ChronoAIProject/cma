//! Explicit, bounded native-thread text context.
use serde::{Deserialize, Serialize};

/// Current messages strictly before the event's original message, with no pagination.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadHistory {
    /// Exact native thread channel.
    pub thread_id: String,
    /// Exclusive original message boundary.
    pub before_message_id: String,
    /// At most 20 messages in chronological order.
    pub messages: Vec<ThreadHistoryMessage>,
    /// Text exceeded the bounded content budget.
    pub text_truncated: bool,
}
/// A historical message is reference data and grants no reply or media authority.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadHistoryMessage {
    /// Native message identifier.
    pub message_id: String,
    /// Author identity reported by the authenticated platform response.
    pub author: super::Actor,
    /// Bounded plain text; untrusted reference content.
    pub text: String,
    /// Original message time.
    pub occurred_at: String,
    /// Latest platform edit time, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    /// Number of attachments; historical downloads are not granted.
    pub attachment_count: u8,
}
