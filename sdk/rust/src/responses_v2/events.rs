//! Typed v2 lifecycle and durable output events.
use super::{OutputMessage, OutputText, ResponseObject};
use serde::Deserialize;

/// A standard event with a durable sequence, or a connection-only observation error.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ResponseEvent {
    /// Durable sequence; connection errors report the next sequence without consuming it.
    pub sequence_number: u64,
    /// Type-specific typed payload.
    #[serde(flatten)]
    pub payload: ResponseEventPayload,
}
/// Standard message projection; provider tools remain in the AG-UI companion.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseEventPayload {
    /// Durable admission, before binding a runtime turn.
    #[serde(rename = "response.created")]
    Created {
        /// Queued resource.
        response: ResponseObject,
    },
    /// Bound runtime turn.
    #[serde(rename = "response.in_progress")]
    InProgress {
        /// Running resource.
        response: ResponseObject,
    },
    /// Durable successful outcome.
    #[serde(rename = "response.completed")]
    Completed {
        /// Completed resource.
        response: ResponseObject,
    },
    /// Durable failure outcome.
    #[serde(rename = "response.failed")]
    Failed {
        /// Failed resource.
        response: ResponseObject,
    },
    /// Durable interrupted outcome.
    #[serde(rename = "response.incomplete")]
    Incomplete {
        /// Incomplete resource.
        response: ResponseObject,
    },
    /// Start one output item.
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        /// Position in output.
        output_index: usize,
        /// Initial message.
        item: OutputMessage,
    },
    /// Finish one output item.
    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        /// Position in output.
        output_index: usize,
        /// Committed message.
        item: OutputMessage,
    },
    /// Start one text content part.
    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        /// Position in output.
        output_index: usize,
        /// Item identity.
        item_id: String,
        /// Position in item content.
        content_index: usize,
        /// Empty text part.
        part: OutputText,
    },
    /// Finish one text content part.
    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        /// Position in output.
        output_index: usize,
        /// Item identity.
        item_id: String,
        /// Position in item content.
        content_index: usize,
        /// Committed text part.
        part: OutputText,
    },
    /// Committed message text fragment.
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        /// Position in output.
        output_index: usize,
        /// Item identity.
        item_id: String,
        /// Position in item content.
        content_index: usize,
        /// Appended text.
        delta: String,
        /// Standard log probabilities.
        logprobs: Vec<serde_json::Value>,
    },
    /// Completed output text.
    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        /// Position in output.
        output_index: usize,
        /// Item identity.
        item_id: String,
        /// Position in item content.
        content_index: usize,
        /// Committed text.
        text: String,
        /// Standard log probabilities.
        logprobs: Vec<serde_json::Value>,
    },
    /// Observation failed without changing the durable command outcome.
    #[serde(rename = "error")]
    Error {
        /// Stable observation error code.
        code: String,
        /// Safe explanation.
        message: String,
        /// Related parameter when present.
        param: Option<String>,
    },
}
impl ResponseEventPayload {
    /// Exact wire event discriminator.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Created { .. } => "response.created",
            Self::InProgress { .. } => "response.in_progress",
            Self::Completed { .. } => "response.completed",
            Self::Failed { .. } => "response.failed",
            Self::Incomplete { .. } => "response.incomplete",
            Self::OutputItemAdded { .. } => "response.output_item.added",
            Self::OutputItemDone { .. } => "response.output_item.done",
            Self::ContentPartAdded { .. } => "response.content_part.added",
            Self::ContentPartDone { .. } => "response.content_part.done",
            Self::OutputTextDelta { .. } => "response.output_text.delta",
            Self::OutputTextDone { .. } => "response.output_text.done",
            Self::Error { .. } => "error",
        }
    }
    /// Resource carried by a lifecycle event.
    pub fn response(&self) -> Option<&ResponseObject> {
        match self {
            Self::Created { response }
            | Self::InProgress { response }
            | Self::Completed { response }
            | Self::Failed { response }
            | Self::Incomplete { response } => Some(response),
            _ => None,
        }
    }
}
