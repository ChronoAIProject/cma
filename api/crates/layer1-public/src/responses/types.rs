use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

/// Standard conversation resource for the supported chat subset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Conversation {
    pub id: String,
    pub object: &'static str,
    pub created_at: u64,
    pub metadata: BTreeMap<String, String>,
}

/// Standard response lifecycle values emitted by this projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    InProgress,
    Completed,
    Failed,
    Incomplete,
}

/// Standard response error member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResponseError {
    pub code: String,
    pub message: String,
}

/// Token counts reported for one response turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResponseUsage {
    pub input_tokens: u64,
    pub input_tokens_details: InputTokenDetails,
    pub output_tokens: u64,
    pub output_tokens_details: OutputTokenDetails,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct InputTokenDetails {
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OutputTokenDetails {
    pub reasoning_tokens: u64,
}

/// Assistant text content in a response output message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OutputText {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
    pub annotations: Vec<serde_json::Value>,
    pub logprobs: Vec<serde_json::Value>,
}

/// The only durable standard output item in the supported subset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OutputMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub status: &'static str,
    pub role: &'static str,
    pub content: Vec<OutputText>,
}

/// A response resource reconstructed from one durable turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ResponseObject {
    /// Browser page for the actual CMA Agent behind this response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
    pub id: String,
    pub object: &'static str,
    pub created_at: u64,
    pub status: ResponseStatus,
    pub background: bool,
    pub error: Option<ResponseError>,
    pub incomplete_details: Option<serde_json::Value>,
    pub instructions: Option<String>,
    pub max_output_tokens: Option<u64>,
    pub model: String,
    pub output: Vec<OutputMessage>,
    pub parallel_tool_calls: bool,
    pub previous_response_id: Option<String>,
    pub reasoning: serde_json::Value,
    pub store: bool,
    pub temperature: Option<f64>,
    pub text: serde_json::Value,
    pub tool_choice: &'static str,
    pub tools: Vec<serde_json::Value>,
    pub top_p: Option<f64>,
    pub truncation: &'static str,
    pub usage: Option<ResponseUsage>,
    pub metadata: BTreeMap<String, String>,
}

pub(crate) fn response_id(session: &layer2_core::AgentId, turn: u64) -> String {
    format!("resp_{}_{}", session.as_str(), turn)
}

pub(crate) fn message_id(session: &layer2_core::AgentId, turn: u64) -> String {
    format!("msg_{}_{}", session.as_str(), turn)
}
