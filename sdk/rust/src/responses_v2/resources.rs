//! Discovery and standard conversation pages.
use super::{MessageKind, OutputStatus, OutputText};
use crate::responses::{InputText, ListObject};
use serde::Deserialize;

/// Standard durable user or assistant message; no provider tool items.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "role")]
pub enum ConversationItem {
    /// User message.
    #[serde(rename = "user")]
    User {
        /// Message tag.
        #[serde(rename = "type")]
        kind: MessageKind,
        /// Durable item ID.
        id: String,
        /// Completed status.
        status: OutputStatus,
        /// User text parts.
        content: Vec<InputText>,
    },
    /// Assistant message.
    #[serde(rename = "assistant")]
    Assistant {
        /// Message tag.
        #[serde(rename = "type")]
        kind: MessageKind,
        /// Durable item ID.
        id: String,
        /// Completed status.
        status: OutputStatus,
        /// Assistant text parts.
        content: Vec<OutputText>,
    },
}
impl ConversationItem {
    /// Durable pagination identity.
    pub fn id(&self) -> &str {
        match self {
            Self::User { id, .. } | Self::Assistant { id, .. } => id,
        }
    }
}
/// Standard list of committed conversation messages.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConversationItems {
    /// Standard list tag.
    pub object: ListObject,
    /// Messages in the requested order.
    pub data: Vec<ConversationItem>,
    /// First item ID, or null for an empty page.
    pub first_id: Option<String>,
    /// Last item ID, or null for an empty page.
    pub last_id: Option<String>,
    /// Whether further messages remain.
    pub has_more: bool,
}
string_enum! { /// Conversation item order.
    pub enum ItemOrder { Asc => "asc", Desc => "desc" }
}
/// Pagination over one existing Agent conversation.
#[derive(Debug, Clone, Default)]
pub struct ItemOptions {
    /// Exclusive cursor in requested order.
    pub after: Option<String>,
    /// Exclusive upper cursor in requested order.
    pub before: Option<String>,
    /// Newest first by default.
    pub order: Option<ItemOrder>,
    /// Page size from 1 through 100.
    pub limit: Option<u16>,
}
string_enum! { /// Discovery document tag.
    pub enum CapabilitiesObject { Capabilities => "cma.responses.capabilities" }
}
/// Documented Responses v2 subset and independently authorized companion.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Capabilities {
    /// Document type.
    pub object: CapabilitiesObject,
    /// Contract version.
    pub version: u64,
    /// Canonical create path.
    pub create: String,
    /// Per-Agent create template.
    pub agent_alias: String,
    /// Response retrieval template.
    pub retrieve: String,
    /// Conversation template.
    pub conversation: String,
    /// Items template.
    pub items: String,
    /// Replay extension.
    pub replay: ReplayCapabilities,
    /// Accepted creation subset.
    pub creation: CreationCapabilities,
    /// Supported output projection.
    pub output: OutputCapabilities,
    /// AG-UI companion template.
    pub companion: String,
    /// Human-readable documentation.
    pub documentation: String,
}
/// CMA replay extension discovery.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReplayCapabilities {
    /// Response event path template.
    pub path: String,
    /// Query cursor name.
    pub cursor: String,
    /// Equivalent header name.
    pub header: String,
    /// Whether event IDs are durable.
    pub durable: bool,
}
/// Supported creation policy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreationCapabilities {
    /// Required stream value.
    pub stream: bool,
    /// Existing-Agent selection policy.
    pub conversation: String,
    /// Predecessor support.
    pub previous_response_id: bool,
    /// Required storage policy.
    pub store: bool,
    /// Supported input form.
    pub input: String,
    /// Pinned-model policy.
    pub model: String,
    /// Optional stable retry key header.
    pub idempotency_header: String,
}
/// Durable output projection discovery.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OutputCapabilities {
    /// Supported standard item types.
    pub standard_items: Vec<String>,
    /// Whether text originates from committed messages or live provider fragments.
    pub text_deltas: String,
    /// Provider tool projection location.
    pub tools: String,
}
