//! CMA's supported Responses chat subset and durable conversation resources.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

string_enum! { /// Durable response lifecycle.
    pub enum ResponseStatus { InProgress => "in_progress", Completed => "completed", Failed => "failed", Incomplete => "incomplete" }
}
string_enum! { /// Durable or live output-item lifecycle.
    pub enum ItemStatus { InProgress => "in_progress", Completed => "completed", Incomplete => "incomplete" }
}
string_enum! { /// Conversation message author.
    pub enum MessageRole { User => "user", Assistant => "assistant" }
}
string_enum! { /// Supported text part type.
    pub enum TextKind { InputText => "input_text", OutputText => "output_text", SummaryText => "summary_text" }
}
string_enum! { /// Supported object discriminant.
    pub enum ConversationObject { Conversation => "conversation" }
}
string_enum! { /// Supported response object discriminant.
    pub enum ResponseObjectKind { Response => "response" }
}
string_enum! { /// Supported item-list discriminant.
    pub enum ListObject { List => "list" }
}
string_enum! { /// Tool execution is not exposed by this chat projection.
    pub enum ToolChoice { None => "none" }
}
string_enum! { /// The current projection does not truncate conversation history.
    pub enum Truncation { Disabled => "disabled" }
}

/// Owned context overrides accepted when creating a conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationMetadata {
    /// Optional owned Agent profile identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_profile: Option<String>,
    /// Optional existing owned Workspace identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Optional existing owned Sandbox identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
}

/// Explicit request to create a conversation in an owned CMA environment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateConversationRequest {
    /// Optional owned hierarchy overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ConversationMetadata>,
}

/// Standard conversation identity projected from a CMA Agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Conversation {
    /// Existing Agent identity.
    pub id: String,
    /// Standard object tag.
    pub object: ConversationObject,
    /// Creation time in epoch seconds.
    pub created_at: u64,
    /// Public hierarchy identity and optional owner title.
    pub metadata: BTreeMap<String, String>,
}

/// Text part in a conversation or response output.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TextPart {
    /// Standard part type.
    #[serde(rename = "type")]
    pub kind: TextKind,
    /// Plain text content.
    pub text: String,
    /// Output annotations, absent for user input.
    #[serde(default)]
    pub annotations: Vec<serde_json::Value>,
    /// Output token probability data, absent for user input.
    #[serde(default)]
    pub logprobs: Vec<serde_json::Value>,
}

/// Standard message item or explicit CMA tool-activity extension.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type")]
pub enum ConversationItem {
    /// Durable user or assistant message.
    #[serde(rename = "message")]
    Message {
        /// Item identity.
        id: String,
        /// Item lifecycle.
        status: ItemStatus,
        /// Message author.
        role: MessageRole,
        /// Text content parts.
        content: Vec<TextPart>,
    },
    /// Read-only tool activity, never an executable tool call.
    #[serde(rename = "cma_chat.tool_activity")]
    ToolActivity {
        /// Item identity.
        id: String,
        /// Runtime call identity.
        call_id: String,
        /// Provider-normalized activity kind.
        kind: String,
        /// Safe activity target.
        target: Option<String>,
        /// Provider-normalized status.
        status: String,
        /// Duration when measured.
        duration_ms: Option<u64>,
    },
}

impl ConversationItem {
    /// Exact item identity used for opaque `after` pagination.
    pub fn id(&self) -> &str {
        match self {
            Self::Message { id, .. } | Self::ToolActivity { id, .. } => id,
        }
    }
}

/// Newest-first durable conversation items.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConversationItems {
    /// Standard list tag.
    pub object: ListObject,
    /// Items in server-defined newest-first order.
    pub data: Vec<ConversationItem>,
    /// First returned item identity.
    pub first_id: Option<String>,
    /// Last returned item identity.
    pub last_id: Option<String>,
    /// Whether another page exists.
    pub has_more: bool,
}

/// Safe durable response failure.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResponseError {
    /// Stable code.
    pub code: String,
    /// Safe detail.
    pub message: String,
}

/// Input-token detail.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InputTokenDetails {
    /// Reused prompt tokens.
    pub cached_tokens: u64,
}

/// Output-token detail.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OutputTokenDetails {
    /// Reasoning output tokens.
    pub reasoning_tokens: u64,
}

/// Standard token usage for one durable turn.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResponseUsage {
    /// Input token count.
    pub input_tokens: u64,
    /// Input token breakdown.
    pub input_tokens_details: InputTokenDetails,
    /// Output token count.
    pub output_tokens: u64,
    /// Output token breakdown.
    pub output_tokens_details: OutputTokenDetails,
    /// Combined token count.
    pub total_tokens: u64,
}

/// Durable response resource reconstructed from one Agent turn.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ResponseObject {
    /// Browser page for the actual CMA Agent, when available.
    #[serde(default)]
    pub callback_url: Option<String>,
    /// Standard response identity.
    pub id: String,
    /// Standard object tag.
    pub object: ResponseObjectKind,
    /// Creation time in epoch seconds.
    pub created_at: u64,
    /// Durable lifecycle.
    pub status: ResponseStatus,
    /// Background execution is not available in this subset.
    pub background: bool,
    /// Safe failure, when the response failed.
    pub error: Option<ResponseError>,
    /// Standard incomplete-response detail, when applicable.
    pub incomplete_details: Option<serde_json::Value>,
    /// Additional instructions are not accepted by this frozen-profile projection.
    pub instructions: Option<String>,
    /// Optional standard output limit.
    pub max_output_tokens: Option<u64>,
    /// Actual model from the frozen Agent profile.
    pub model: String,
    /// Durable standard output messages.
    pub output: Vec<ConversationItem>,
    /// Standard parallel tool-call flag.
    pub parallel_tool_calls: bool,
    /// Previous response identity when provided.
    pub previous_response_id: Option<String>,
    /// Standard reasoning metadata; reasoning text is live-only.
    pub reasoning: serde_json::Value,
    /// Durable storage is required by this projection.
    pub store: bool,
    /// Standard sampling metadata.
    pub temperature: Option<f64>,
    /// Standard text-format metadata.
    pub text: serde_json::Value,
    /// Explicit absence of executable tools.
    pub tool_choice: ToolChoice,
    /// Standard tool declarations; empty for this subset.
    pub tools: Vec<serde_json::Value>,
    /// Standard sampling metadata.
    pub top_p: Option<f64>,
    /// Explicitly disabled truncation.
    pub truncation: Truncation,
    /// Durable token usage, when available.
    pub usage: Option<ResponseUsage>,
    /// Caller metadata retained on the turn.
    pub metadata: BTreeMap<String, String>,
}

/// Supported response admission request; no stateless, tool, or background fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateResponseRequest {
    /// Existing Agent conversation identity.
    pub conversation: String,
    /// Plain text or exactly one supported standard user message.
    pub input: super::ResponseInput,
    /// Must be true; this projection requires streaming.
    pub stream: bool,
    /// Caller metadata, bounded by the server.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// Optional prior response identity in this conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    /// Optional compatibility model hint; the server uses the frozen profile model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// When supplied, must be true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
}

impl CreateResponseRequest {
    /// Construct the supported streaming request for one existing conversation.
    pub fn new(conversation: impl Into<String>, input: impl Into<super::ResponseInput>) -> Self {
        Self {
            conversation: conversation.into(),
            input: input.into(),
            stream: true,
            metadata: BTreeMap::new(),
            previous_response_id: None,
            model: None,
            store: None,
        }
    }
}
