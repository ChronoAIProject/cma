//! Stateful OpenAI Responses API chat projection.

use std::collections::BTreeMap;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use eventsource_stream::{EventStreamError, Eventsource};
use futures_util::{Stream, StreamExt};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Client, Error, Result, SessionId};

const MAX_EVENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ITEMS_LIMIT: u16 = 100;

/// A stateful, deployment-managed chat conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponsesConversation {
    /// CMA session identifier used as the conversation identifier.
    pub id: SessionId,
    /// Standard object discriminator. CMA emits `conversation`.
    pub object: String,
    /// Unix timestamp at which the durable session was created.
    pub created_at: u64,
    /// Server-owned metadata, including the retained title when present.
    pub metadata: BTreeMap<String, String>,
}

impl ResponsesConversation {
    fn validate(self) -> Result<Self> {
        if self.object != "conversation" {
            return Err(Error::protocol(
                "Responses conversation object was not 'conversation'",
            ));
        }
        Ok(self)
    }
}

/// Lifecycle states represented by a response resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    /// The admitted turn has not reached a durable terminal boundary.
    InProgress,
    /// The turn completed successfully.
    Completed,
    /// The turn ended with a typed failure.
    Failed,
    /// The turn ended before producing a complete answer.
    Incomplete,
}

/// A typed error attached to a failed response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseError {
    /// Stable CMA problem code.
    pub code: String,
    /// Human-readable, secret-safe failure detail.
    pub message: String,
}

/// Input-token detail retained by CMA.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseInputTokenDetails {
    /// Cached input tokens, currently zero when unavailable.
    pub cached_tokens: u64,
}

/// Output-token detail retained by CMA.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseOutputTokenDetails {
    /// Reasoning tokens, currently zero when unavailable.
    pub reasoning_tokens: u64,
}

/// Token accounting for one response turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseUsage {
    /// Input tokens consumed by the turn.
    pub input_tokens: u64,
    /// Detail for the input-token count.
    pub input_tokens_details: ResponseInputTokenDetails,
    /// Output tokens produced by the turn.
    pub output_tokens: u64,
    /// Detail for the output-token count.
    pub output_tokens_details: ResponseOutputTokenDetails,
    /// Sum of input and output tokens.
    pub total_tokens: u64,
}

/// One completed assistant output-text part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseOutputText {
    /// Standard part discriminator. CMA emits `output_text`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Complete assistant text.
    pub text: String,
    /// Standard annotation lane, empty in CMA v1.
    pub annotations: Vec<Value>,
    /// Standard log-probability lane, empty in CMA v1.
    pub logprobs: Vec<Value>,
}

/// One completed assistant message in a response output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseOutputMessage {
    /// Stable message identifier derived from the durable turn.
    pub id: String,
    /// Standard item discriminator. CMA emits `message`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Item status. Durable response messages are completed.
    pub status: String,
    /// Message role. CMA response output is assistant-authored.
    pub role: String,
    /// Completed output-text parts.
    pub content: Vec<ResponseOutputText>,
}

/// A response reconstructed from one durable conversation turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponsesResponse {
    /// Stable `resp_<session>_<turn>` identifier.
    pub id: String,
    /// Standard object discriminator. CMA emits `response`.
    pub object: String,
    /// Unix timestamp of the durable turn start.
    pub created_at: u64,
    /// Current or terminal lifecycle status.
    pub status: ResponseStatus,
    /// Background responses are outside CMA v1 and this remains false.
    pub background: bool,
    /// Failure detail for a failed response.
    pub error: Option<ResponseError>,
    /// Standard incomplete detail when the response is incomplete.
    pub incomplete_details: Option<Value>,
    /// Instructions are outside CMA's subset and remain absent.
    pub instructions: Option<String>,
    /// Caller output limits are outside CMA's subset and remain absent.
    pub max_output_tokens: Option<u64>,
    /// Deployment-configured chat model.
    pub model: String,
    /// Durable assistant output messages.
    pub output: Vec<ResponseOutputMessage>,
    /// Parallel caller tools are unavailable and this remains false.
    pub parallel_tool_calls: bool,
    /// Previous durable response in this linear conversation.
    pub previous_response_id: Option<String>,
    /// Standard reasoning configuration shell; no durable reasoning text.
    pub reasoning: Value,
    /// CMA chat responses are durable and this remains true.
    pub store: bool,
    /// Sampling temperature is deployment-owned and remains absent.
    pub temperature: Option<f64>,
    /// Standard text-output configuration projection.
    pub text: Value,
    /// Caller tool selection is unavailable and this remains `none`.
    pub tool_choice: String,
    /// Caller tools are unavailable and this remains empty.
    pub tools: Vec<Value>,
    /// Sampling top-p is deployment-owned and remains absent.
    pub top_p: Option<f64>,
    /// CMA does not truncate a linear chat fork and emits `disabled`.
    pub truncation: String,
    /// Token accounting once reported by the runtime.
    pub usage: Option<ResponseUsage>,
    /// Caller-supplied turn metadata retained in the durable log.
    pub metadata: BTreeMap<String, String>,
}

impl ResponsesResponse {
    fn validate(self) -> Result<Self> {
        if self.object != "response"
            || self.id.is_empty()
            || self.model.is_empty()
            || self.background
            || self.parallel_tool_calls
            || !self.store
            || self.tool_choice != "none"
            || self.truncation != "disabled"
        {
            return Err(Error::protocol(
                "Responses response violated the declared CMA wire subset",
            ));
        }
        for output in &self.output {
            if output.id.is_empty()
                || output.kind != "message"
                || output.role != "assistant"
                || output.status != "completed"
                || output.content.iter().any(|part| part.kind != "output_text")
            {
                return Err(Error::protocol(
                    "Responses output message violated the declared CMA wire subset",
                ));
            }
        }
        if let Some(usage) = &self.usage
            && usage.total_tokens != usage.input_tokens.saturating_add(usage.output_tokens)
        {
            return Err(Error::protocol(
                "Responses usage total did not equal input plus output tokens",
            ));
        }
        Ok(self)
    }
}

/// One bounded body attached to CMA tool activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseToolBody {
    /// Recorded body text.
    pub text: String,
    /// Whether recording policy truncated the body.
    pub truncated: bool,
}

/// CMA's namespaced durable or live tool activity item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseToolActivity {
    /// Stable tool item identifier.
    pub id: String,
    /// CMA extension discriminator.
    #[serde(rename = "type")]
    pub kind: String,
    /// Runtime tool-call identity.
    pub call_id: String,
    /// Tool category recorded by the runtime.
    #[serde(rename = "kind")]
    pub tool_kind: String,
    /// Human-readable tool target.
    pub target: String,
    /// Latest recorded tool status.
    pub status: String,
    /// Milliseconds from start to the latest update when known.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Bounded recorded arguments when policy retained them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<ResponseToolBody>,
    /// Bounded recorded result when policy retained it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ResponseToolBody>,
}

/// One input- or output-text part in durable history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationMessageContent {
    /// Standard content discriminator.
    #[serde(rename = "type")]
    pub kind: String,
    /// Complete retained text.
    pub text: String,
    /// Output annotations when this is assistant text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Vec<Value>>,
    /// Output log probabilities when this is assistant text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<Vec<Value>>,
}

/// One standard durable user or assistant message item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationMessageItem {
    /// Stable item identifier.
    pub id: String,
    /// Standard item discriminator.
    #[serde(rename = "type")]
    pub kind: String,
    /// Durable item status.
    pub status: String,
    /// User or assistant role.
    pub role: String,
    /// Complete text parts.
    pub content: Vec<ConversationMessageContent>,
}

/// Supported standard and CMA-extension history items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponsesConversationItem {
    /// A standard user or assistant message.
    Message(ConversationMessageItem),
    /// CMA's namespaced tool activity extension.
    ToolActivity(ResponseToolActivity),
}

/// OpenAI list envelope for one newest-first history page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponsesConversationItemList {
    /// Standard list discriminator.
    pub object: String,
    /// Newest-first retained items.
    pub data: Vec<ResponsesConversationItem>,
    /// First returned item identifier.
    pub first_id: Option<String>,
    /// Last returned item identifier.
    pub last_id: Option<String>,
    /// Whether older items remain after this page.
    pub has_more: bool,
}

impl ResponsesConversationItemList {
    fn validate(self) -> Result<Self> {
        if self.object != "list"
            || self.first_id.is_some() == self.data.is_empty()
            || self.last_id.is_some() == self.data.is_empty()
        {
            return Err(Error::protocol(
                "Responses conversation item list violated its envelope",
            ));
        }
        Ok(self)
    }
}

/// One input-text content part accepted by CMA.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResponseInputText {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
}

impl ResponseInputText {
    /// Build one non-empty standard input-text part.
    pub fn new(text: impl Into<String>) -> Result<Self> {
        let text = text.into();
        if text.is_empty() {
            return Err(Error::protocol("Responses input text must not be empty"));
        }
        Ok(Self {
            kind: "input_text",
            text,
        })
    }
}

/// Message content accepted by CMA's single-user-message input subset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ResponseInputContent {
    /// A direct non-empty text string.
    Text(String),
    /// Exactly one standard input-text part.
    Parts(Vec<ResponseInputText>),
}

/// One standard user input message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResponseInputMessage {
    #[serde(rename = "type")]
    kind: &'static str,
    role: &'static str,
    content: ResponseInputContent,
}

impl ResponseInputMessage {
    /// Build a user message whose content is a direct text string.
    pub fn text(text: impl Into<String>) -> Result<Self> {
        let text = text.into();
        if text.is_empty() {
            return Err(Error::protocol("Responses input text must not be empty"));
        }
        Ok(Self {
            kind: "message",
            role: "user",
            content: ResponseInputContent::Text(text),
        })
    }

    /// Build a user message with one standard input-text content part.
    pub fn input_text(text: impl Into<String>) -> Result<Self> {
        Ok(Self {
            kind: "message",
            role: "user",
            content: ResponseInputContent::Parts(vec![ResponseInputText::new(text)?]),
        })
    }
}

/// Input accepted by response creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ResponseInput {
    /// A direct non-empty user text string.
    Text(String),
    /// Exactly one standard user message.
    Messages(Vec<ResponseInputMessage>),
}

/// Honored and explicitly accepted response-creation fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateResponseRequest {
    /// Required stateful chat conversation.
    pub conversation: SessionId,
    /// Exactly one user text input.
    pub input: ResponseInput,
    /// Caller metadata retained with the turn.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// Linear predecessor fence for every turn after the first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    /// Caller model hint; CMA echoes its configured model instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional explicit durable-store acknowledgement; only true is valid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
}

impl CreateResponseRequest {
    /// Build a response request with direct text input.
    pub fn text(conversation: SessionId, text: impl Into<String>) -> Result<Self> {
        let text = text.into();
        if text.is_empty() {
            return Err(Error::protocol("Responses input text must not be empty"));
        }
        Ok(Self {
            conversation,
            input: ResponseInput::Text(text),
            metadata: BTreeMap::new(),
            previous_response_id: None,
            model: None,
            store: None,
        })
    }

    /// Build a response request with one standard user message input.
    pub fn message(conversation: SessionId, message: ResponseInputMessage) -> Self {
        Self {
            conversation,
            input: ResponseInput::Messages(vec![message]),
            metadata: BTreeMap::new(),
            previous_response_id: None,
            model: None,
            store: None,
        }
    }

    pub(crate) fn wire_body(&self) -> Result<Value> {
        match &self.input {
            ResponseInput::Text(text) if text.is_empty() => {
                return Err(Error::protocol("Responses input text must not be empty"));
            }
            ResponseInput::Messages(messages) if messages.len() != 1 => {
                return Err(Error::protocol(
                    "Responses input must contain exactly one user message",
                ));
            }
            _ => {}
        }
        if self.model.as_ref().is_some_and(String::is_empty) {
            return Err(Error::protocol("Responses model hint must not be empty"));
        }
        if self.store == Some(false) {
            return Err(Error::protocol(
                "Responses store may only be omitted or true",
            ));
        }
        let mut body = serde_json::to_value(self).map_err(|error| {
            Error::protocol(format!("could not encode Responses request: {error}"))
        })?;
        body.as_object_mut()
            .expect("a response request serializes as an object")
            .insert("stream".into(), true.into());
        Ok(body)
    }
}

/// Pagination controls for newest-first conversation items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationItemsRequest {
    /// Return items older than this same-conversation item identifier.
    pub after: Option<String>,
    /// Page size from one through one hundred.
    pub limit: Option<u16>,
}

impl ConversationItemsRequest {
    fn query(&self) -> Result<Vec<(&'static str, String)>> {
        if self.after.as_ref().is_some_and(String::is_empty) {
            return Err(Error::protocol(
                "Responses conversation item cursor must not be empty",
            ));
        }
        if self
            .limit
            .is_some_and(|limit| !(1..=MAX_ITEMS_LIMIT).contains(&limit))
        {
            return Err(Error::protocol(
                "Responses conversation item limit must be from 1 through 100",
            ));
        }
        let mut query = Vec::new();
        if let Some(after) = &self.after {
            query.push(("after", after.clone()));
        }
        if let Some(limit) = self.limit {
            query.push(("limit", limit.to_string()));
        }
        Ok(query)
    }
}

/// One event in CMA's declared Responses stream subset.
///
/// Named fields intentionally retain the standard snake-case wire vocabulary.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponsesEvent {
    #[serde(rename = "response.created")]
    Created {
        sequence_number: u64,
        response: ResponsesResponse,
    },
    #[serde(rename = "response.in_progress")]
    InProgress {
        sequence_number: u64,
        response: ResponsesResponse,
    },
    #[serde(rename = "response.reasoning_summary_part.added")]
    ReasoningSummaryPartAdded {
        sequence_number: u64,
        item_id: String,
        output_index: u64,
        summary_index: u64,
        part: Value,
    },
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta {
        sequence_number: u64,
        item_id: String,
        output_index: u64,
        summary_index: u64,
        delta: String,
    },
    #[serde(rename = "response.reasoning_summary_text.done")]
    ReasoningSummaryTextDone {
        sequence_number: u64,
        item_id: String,
        output_index: u64,
        summary_index: u64,
        text: String,
    },
    #[serde(rename = "response.reasoning_summary_part.done")]
    ReasoningSummaryPartDone {
        sequence_number: u64,
        item_id: String,
        output_index: u64,
        summary_index: u64,
        part: Value,
    },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        sequence_number: u64,
        output_index: u64,
        item: Value,
    },
    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        sequence_number: u64,
        item_id: String,
        output_index: u64,
        content_index: u64,
        part: Value,
    },
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        sequence_number: u64,
        item_id: String,
        output_index: u64,
        content_index: u64,
        delta: String,
        logprobs: Vec<Value>,
    },
    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        sequence_number: u64,
        item_id: String,
        output_index: u64,
        content_index: u64,
        text: String,
        logprobs: Vec<Value>,
    },
    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        sequence_number: u64,
        item_id: String,
        output_index: u64,
        content_index: u64,
        part: Value,
    },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        sequence_number: u64,
        output_index: u64,
        item: Value,
    },
    #[serde(rename = "response.cma_chat.tool_activity")]
    ToolActivity {
        sequence_number: u64,
        response_id: String,
        item: ResponseToolActivity,
    },
    #[serde(rename = "response.completed")]
    Completed {
        sequence_number: u64,
        response: ResponsesResponse,
    },
    #[serde(rename = "response.failed")]
    Failed {
        sequence_number: u64,
        response: ResponsesResponse,
    },
    #[serde(rename = "response.incomplete")]
    Incomplete {
        sequence_number: u64,
        response: ResponsesResponse,
    },
}

impl ResponsesEvent {
    /// Zero-based sequence number within this response stream.
    pub fn sequence_number(&self) -> u64 {
        match self {
            Self::Created {
                sequence_number, ..
            }
            | Self::InProgress {
                sequence_number, ..
            }
            | Self::ReasoningSummaryPartAdded {
                sequence_number, ..
            }
            | Self::ReasoningSummaryTextDelta {
                sequence_number, ..
            }
            | Self::ReasoningSummaryTextDone {
                sequence_number, ..
            }
            | Self::ReasoningSummaryPartDone {
                sequence_number, ..
            }
            | Self::OutputItemAdded {
                sequence_number, ..
            }
            | Self::ContentPartAdded {
                sequence_number, ..
            }
            | Self::OutputTextDelta {
                sequence_number, ..
            }
            | Self::OutputTextDone {
                sequence_number, ..
            }
            | Self::ContentPartDone {
                sequence_number, ..
            }
            | Self::OutputItemDone {
                sequence_number, ..
            }
            | Self::ToolActivity {
                sequence_number, ..
            }
            | Self::Completed {
                sequence_number, ..
            }
            | Self::Failed {
                sequence_number, ..
            }
            | Self::Incomplete {
                sequence_number, ..
            } => *sequence_number,
        }
    }

    /// Wire event name repeated in the SSE `event` field.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::Created { .. } => "response.created",
            Self::InProgress { .. } => "response.in_progress",
            Self::ReasoningSummaryPartAdded { .. } => "response.reasoning_summary_part.added",
            Self::ReasoningSummaryTextDelta { .. } => "response.reasoning_summary_text.delta",
            Self::ReasoningSummaryTextDone { .. } => "response.reasoning_summary_text.done",
            Self::ReasoningSummaryPartDone { .. } => "response.reasoning_summary_part.done",
            Self::OutputItemAdded { .. } => "response.output_item.added",
            Self::ContentPartAdded { .. } => "response.content_part.added",
            Self::OutputTextDelta { .. } => "response.output_text.delta",
            Self::OutputTextDone { .. } => "response.output_text.done",
            Self::ContentPartDone { .. } => "response.content_part.done",
            Self::OutputItemDone { .. } => "response.output_item.done",
            Self::ToolActivity { .. } => "response.cma_chat.tool_activity",
            Self::Completed { .. } => "response.completed",
            Self::Failed { .. } => "response.failed",
            Self::Incomplete { .. } => "response.incomplete",
        }
    }

    /// Whether this event closes the response connection.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Incomplete { .. }
        )
    }

    fn validate(self) -> Result<Self> {
        match &self {
            Self::Created { response, .. }
            | Self::InProgress { response, .. }
            | Self::Completed { response, .. }
            | Self::Failed { response, .. }
            | Self::Incomplete { response, .. } => {
                response.clone().validate()?;
            }
            Self::ToolActivity { item, .. } if item.kind != "cma_chat.tool_activity" => {
                return Err(Error::protocol(
                    "Responses tool event carried the wrong extension discriminator",
                ));
            }
            _ => {}
        }
        Ok(self)
    }
}

/// A validated, single-attempt stream for one admitted response.
///
/// Dropping this stream closes the local HTTP body. It never issues a response
/// cancellation request and therefore does not cancel CMA's core turn.
pub struct ResponsesStream {
    inner: Pin<Box<dyn Stream<Item = Result<ResponsesEvent>> + Send + 'static>>,
}

impl ResponsesStream {
    pub(crate) async fn open(client: Client, request: CreateResponseRequest) -> Result<Self> {
        let response = client.connect_response(&request).await?;
        Ok(Self {
            inner: Box::pin(response_events(response)),
        })
    }

    /// Wait for the next event or stream failure.
    pub async fn next(&mut self) -> Option<Result<ResponsesEvent>> {
        StreamExt::next(self).await
    }
}

impl fmt::Debug for ResponsesStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResponsesStream")
            .field("connection", &"[streaming]")
            .finish()
    }
}

impl Stream for ResponsesStream {
    type Item = Result<ResponsesEvent>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(context)
    }
}

fn response_events(
    response: reqwest::Response,
) -> impl Stream<Item = Result<ResponsesEvent>> + Send + 'static {
    async_stream::stream! {
        let mut source = response.bytes_stream().eventsource();
        let mut expected_sequence = 0_u64;
        while let Some(frame) = source.next().await {
            let frame = match frame {
                Ok(frame) => frame,
                Err(EventStreamError::Transport(error)) => {
                    yield Err(Error::transport(format!("Responses stream transport failed: {error}")));
                    return;
                }
                Err(EventStreamError::Utf8(error)) => {
                    yield Err(Error::protocol(format!("Responses stream was not valid UTF-8: {error}")));
                    return;
                }
                Err(EventStreamError::Parser(error)) => {
                    yield Err(Error::protocol(format!("Responses stream was not valid SSE: {error}")));
                    return;
                }
            };
            if frame.data.len() > MAX_EVENT_BYTES {
                yield Err(Error::protocol(format!(
                    "Responses event exceeded the {MAX_EVENT_BYTES}-byte boundary"
                )));
                return;
            }
            let event = match serde_json::from_str::<ResponsesEvent>(&frame.data) {
                Ok(event) => match event.validate() {
                    Ok(event) => event,
                    Err(error) => {
                        yield Err(error);
                        return;
                    }
                },
                Err(error) => {
                    yield Err(Error::protocol(format!(
                        "Responses frame did not match the declared wire shape: {error}"
                    )));
                    return;
                }
            };
            if frame.event != "message" && frame.event != event.event_type() {
                yield Err(Error::protocol(format!(
                    "Responses SSE event '{}' did not match payload type '{}'",
                    frame.event,
                    event.event_type()
                )));
                return;
            }
            if event.sequence_number() != expected_sequence {
                yield Err(Error::protocol(format!(
                    "Responses stream expected sequence {expected_sequence} but received {}",
                    event.sequence_number()
                )));
                return;
            }
            expected_sequence = expected_sequence.saturating_add(1);
            let terminal = event.is_terminal();
            yield Ok(event);
            if terminal {
                return;
            }
        }
        yield Err(Error::protocol(
            "Responses stream closed before a terminal event",
        ));
    }
}

impl Client {
    /// Create a deployment-managed chat conversation.
    pub async fn create_conversation(&self) -> Result<ResponsesConversation> {
        let conversation: ResponsesConversation = self
            .request_public_json(
                Method::POST,
                &["conversations"],
                &[],
                Some(serde_json::json!({})),
                StatusCode::OK,
                false,
            )
            .await?;
        conversation.validate()
    }

    /// Read one caller-owned chat conversation.
    pub async fn read_conversation(&self, id: &SessionId) -> Result<ResponsesConversation> {
        let conversation: ResponsesConversation = self
            .request_public_json(
                Method::GET,
                &["conversations", id.as_str()],
                &[],
                None,
                StatusCode::OK,
                false,
            )
            .await?;
        conversation.validate()
    }

    /// Read one newest-first page of durable conversation history.
    pub async fn list_conversation_items(
        &self,
        id: &SessionId,
        page: ConversationItemsRequest,
    ) -> Result<ResponsesConversationItemList> {
        let query = page.query()?;
        let list: ResponsesConversationItemList = self
            .request_public_json(
                Method::GET,
                &["conversations", id.as_str(), "items"],
                &query,
                None,
                StatusCode::OK,
                false,
            )
            .await?;
        list.validate()
    }

    /// Retrieve one response reconstructed from durable history.
    pub async fn read_response(&self, id: &str) -> Result<ResponsesResponse> {
        validate_response_id(id)?;
        let response: ResponsesResponse = self
            .request_public_json(
                Method::GET,
                &["responses", id],
                &[],
                None,
                StatusCode::OK,
                false,
            )
            .await?;
        response.validate()
    }

    /// Admit one chat turn and open its non-reconnecting Responses stream.
    pub async fn create_response(&self, request: CreateResponseRequest) -> Result<ResponsesStream> {
        ResponsesStream::open(self.clone(), request).await
    }
}

fn validate_response_id(value: &str) -> Result<()> {
    let Some((session, turn)) = value
        .strip_prefix("resp_")
        .and_then(|tail| tail.rsplit_once('_'))
    else {
        return Err(Error::protocol(
            "a response id must use resp_<session>_<turn>",
        ));
    };
    if !session.starts_with("sesn_")
        || turn.parse::<u64>().is_err()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(Error::protocol(
            "a response id must use resp_<session>_<turn>",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_constructors_keep_the_supported_subset_exact() {
        let conversation = SessionId::new("sesn_fixture").unwrap();
        let mut request = CreateResponseRequest::text(conversation, "continue").unwrap();
        request.previous_response_id = Some("resp_sesn_fixture_1".into());
        request.store = Some(true);
        assert_eq!(
            request.wire_body().unwrap(),
            serde_json::json!({
                "conversation": "sesn_fixture",
                "input": "continue",
                "previous_response_id": "resp_sesn_fixture_1",
                "store": true,
                "stream": true,
            })
        );
    }

    #[test]
    fn invalid_local_boundaries_do_not_become_ambiguous_requests() {
        let conversation = SessionId::new("sesn_fixture").unwrap();
        assert!(CreateResponseRequest::text(conversation.clone(), "").is_err());
        let mut request = CreateResponseRequest::text(conversation, "ok").unwrap();
        request.store = Some(false);
        assert!(request.wire_body().is_err());
        assert!(
            ConversationItemsRequest {
                after: None,
                limit: Some(101),
            }
            .query()
            .is_err()
        );
        assert!(validate_response_id("resp_not-a-session_1").is_err());
    }
}
