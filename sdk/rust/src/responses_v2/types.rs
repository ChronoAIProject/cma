//! Typed durable Responses v2 resources.
pub use crate::responses::Conversation;
use crate::responses::{ResponseError, ResponseObjectKind, ResponseUsage, ToolChoice, Truncation};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
string_enum! { /// Durable v2 response lifecycle.
    pub enum ResponseStatus { Queued => "queued", InProgress => "in_progress", Completed => "completed", Failed => "failed", Incomplete => "incomplete" }
}
string_enum! { /// Output message lifecycle.
    pub enum OutputStatus { InProgress => "in_progress", Completed => "completed" }
}
string_enum! { /// Standard output message author.
    pub enum OutputRole { Assistant => "assistant" }
}
string_enum! { /// Standard output item tag.
    pub enum MessageKind { Message => "message" }
}
string_enum! { /// Standard output text tag.
    pub enum OutputTextKind { OutputText => "output_text" }
}
/// Existing conversation reference in a response resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseConversation {
    /// Agent identity.
    pub id: String,
}
/// Standard output text content.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OutputText {
    /// Standard text tag.
    #[serde(rename = "type")]
    pub kind: OutputTextKind,
    /// Committed assistant text.
    pub text: String,
    /// Standard annotations.
    pub annotations: Vec<serde_json::Value>,
    /// Standard log probabilities.
    pub logprobs: Vec<serde_json::Value>,
}
/// One assistant output message, shared with conversation item identities.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OutputMessage {
    /// Standard message tag.
    #[serde(rename = "type")]
    pub kind: MessageKind,
    /// Durable item identity.
    pub id: String,
    /// Current output lifecycle.
    pub status: OutputStatus,
    /// Assistant role.
    pub role: OutputRole,
    /// Committed text parts.
    pub content: Vec<OutputText>,
}
string_enum! { /// Provider activity item tag.
    pub enum ActivityKind { McpCall => "mcp_call" }
}
string_enum! { /// Provider activity lifecycle.
    pub enum ActivityStatus { InProgress => "in_progress", Completed => "completed", Failed => "failed" }
}
/// Informational provider activity. No executable arguments or result bodies are exposed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ActivityItem {
    /// Standard activity tag.
    #[serde(rename = "type")]
    pub kind: ActivityKind,
    /// Stable call identity.
    pub id: String,
    /// Provider label.
    pub server_label: String,
    /// Display title, bounded by the server to 200 characters.
    pub name: String,
    /// Empty in the activity-only contract.
    pub arguments: String,
    /// Absent in the activity-only contract.
    pub output: Option<String>,
    #[doc = "Bounded reported output, separate from executable tool results."]
    #[serde(rename = "cma:output")]
    pub recorded_output: Option<crate::agents::RecordedToolOutput>,
    /// Absent in the activity-only contract.
    pub error: Option<serde_json::Value>,
    /// Current call lifecycle.
    pub status: ActivityStatus,
}
/// Durable message or informational provider activity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum OutputItem {
    /// Committed assistant text.
    Message(OutputMessage),
    /// Provider activity, never a client tool request.
    Activity(ActivityItem),
}
impl OutputItem {
    /// Stable output identity.
    pub fn id(&self) -> &str {
        match self {
            Self::Message(item) => &item.id,
            Self::Activity(item) => &item.id,
        }
    }
}
/// Durable response resource reconstructed from one Agent turn.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ResponseObject {
    /// Existing Agent conversation retained across every turn.
    pub conversation: ResponseConversation,
    /// Epoch seconds when the durable turn completed, when present.
    pub completed_at: Option<u64>,
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
    /// Durable standard messages and provider activity.
    pub output: Vec<OutputItem>,
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
