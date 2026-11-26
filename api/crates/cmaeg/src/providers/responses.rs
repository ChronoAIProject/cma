//! Typed, forward-compatible OpenAI Responses event projections.

use std::fmt;

use serde_json::Value;

use super::sse::SseFrame;

/// A response object is intentionally retained as JSON for additive provider fields.
pub type ResponseObject = Value;

#[derive(Debug, Clone, PartialEq)]
pub enum ResponsesEvent {
    /// Negotiated diagnostics were consumed separately from model output.
    NyxidTrace,
    Created {
        response_id: String,
        thread_reply: bool,
        native_agent_id: Option<String>,
    },
    InProgress {
        native_agent_id: Option<String>,
    },
    OutputItemAdded {
        index: usize,
        item: OutputItem,
    },
    OutputTextDelta {
        item_id: String,
        delta: String,
    },
    OutputTextDone {
        item_id: String,
        text: String,
    },
    OutputItemDone {
        index: usize,
        item: OutputItem,
    },
    McpCallInProgress {
        item_id: String,
    },
    McpCallCompleted {
        item_id: String,
    },
    McpCallFailed {
        item_id: String,
    },
    Completed {
        response: ResponseObject,
        /// A terminal snapshot must agree with any explicit frame response ID.
        matching_response_identity: bool,
    },
    Failed {
        error: ResponseError,
    },
    Incomplete {
        reason: String,
    },
    Unknown {
        r#type: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputItem {
    Message {
        id: String,
        text_so_far: String,
    },
    McpCall {
        id: String,
        name: String,
        status: String,
    },
    FunctionCall {
        id: String,
        name: String,
        arguments: String,
    },
    Other {
        r#type: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponsesEventError {
    InvalidJson(String),
    MissingType,
    Malformed(String),
}

impl fmt::Display for ResponsesEventError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(detail) => {
                write!(formatter, "provider event JSON is invalid: {detail}")
            }
            Self::MissingType => formatter.write_str("provider event has no type"),
            Self::Malformed(detail) => write!(formatter, "provider event is malformed: {detail}"),
        }
    }
}

impl std::error::Error for ResponsesEventError {}

/// Parse one SSE frame. Unknown event types are deliberately preserved.
pub fn parse_event(frame: &SseFrame) -> Result<ResponsesEvent, ResponsesEventError> {
    parse_event_with_trace(frame, None)
}

pub(crate) fn parse_event_with_trace(
    frame: &SseFrame,
    trace: Option<&mut crate::telemetry::turn_trace::ProviderReader>,
) -> Result<ResponsesEvent, ResponsesEventError> {
    parse_event_for_turn(frame, trace, None).map(|(event, _)| event)
}

pub(super) fn parse_event_for_turn(
    frame: &SseFrame,
    trace: Option<&mut crate::telemetry::turn_trace::ProviderReader>,
    accepted_response: Option<&str>,
) -> Result<(ResponsesEvent, bool), ResponsesEventError> {
    let value = serde_json::from_str::<Value>(&frame.data)
        .map_err(|error| ResponsesEventError::InvalidJson(error.to_string()))?;
    parse_turn_value(value, frame.event.as_deref(), trace, accepted_response)
}

pub(super) fn terminal_snapshot(
    response: Value,
) -> Result<Option<ResponsesEvent>, ResponsesEventError> {
    let kind = match response["status"].as_str() {
        Some("completed") => "response.completed",
        Some("failed") => "response.failed",
        Some("incomplete") => "response.incomplete",
        _ => return Ok(None),
    };
    parse_turn_value(
        serde_json::json!({"response": response}),
        Some(kind),
        None,
        None,
    )
    .map(|(event, _)| Some(event))
}

fn parse_turn_value(
    value: Value,
    event_type: Option<&str>,
    trace: Option<&mut crate::telemetry::turn_trace::ProviderReader>,
    accepted_response: Option<&str>,
) -> Result<(ResponsesEvent, bool), ResponsesEventError> {
    let object = value
        .as_object()
        .ok_or_else(|| ResponsesEventError::Malformed("event must be a JSON object".into()))?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .or(event_type)
        .ok_or(ResponsesEventError::MissingType)?;
    if let Some(trace) = trace {
        // The reader owns immutable diagnostic association. Malformed/conflicting
        // annotations cannot reject an otherwise valid provider event.
        trace.event(&value);
        if kind == "response.nyxid_trace" {
            return Ok((ResponsesEvent::NyxidTrace, true));
        }
    }
    let matching_identity = matches_turn_identity(object, kind, accepted_response);
    match kind {
        "response.created" => Ok(ResponsesEvent::Created {
            response_id: response_id(object)?,
            native_agent_id: native_agent_id(&value),
            thread_reply: value
                .pointer("/response/thread_reply")
                .map(|value| {
                    value.as_bool().ok_or_else(|| {
                        ResponsesEventError::Malformed("invalid native thread reply choice".into())
                    })
                })
                .transpose()?
                .unwrap_or(false),
        }),
        "response.in_progress" => Ok(ResponsesEvent::InProgress {
            native_agent_id: native_agent_id(&value),
        }),
        "response.output_item.added" => Ok(ResponsesEvent::OutputItemAdded {
            index: index(object)?,
            item: output_item(object.get("item"))?,
        }),
        "response.output_text.delta" => Ok(ResponsesEvent::OutputTextDelta {
            item_id: string_field(object, "item_id")?,
            delta: string_field(object, "delta")?,
        }),
        "response.output_text.done" => Ok(ResponsesEvent::OutputTextDone {
            item_id: string_field(object, "item_id")?,
            text: string_field(object, "text")?,
        }),
        "response.output_item.done" => Ok(ResponsesEvent::OutputItemDone {
            index: index(object)?,
            item: output_item(object.get("item"))?,
        }),
        "response.mcp_call.in_progress" => Ok(ResponsesEvent::McpCallInProgress {
            item_id: string_field(object, "item_id")?,
        }),
        "response.mcp_call.completed" => Ok(ResponsesEvent::McpCallCompleted {
            item_id: string_field(object, "item_id")?,
        }),
        "response.mcp_call.failed" => Ok(ResponsesEvent::McpCallFailed {
            item_id: string_field(object, "item_id")?,
        }),
        "response.completed" => {
            let response = object.get("response").unwrap_or(&value);
            let id = response.get("id").and_then(Value::as_str);
            let matching_response_identity = id.is_some()
                && object
                    .get("response_id")
                    .is_none_or(|field| field.as_str() == id);
            Ok(ResponsesEvent::Completed {
                response: response.clone(),
                matching_response_identity,
            })
        }
        "response.failed" => {
            let error = object
                .get("error")
                .or_else(|| {
                    object
                        .get("response")
                        .and_then(|response| response.get("error"))
                })
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    ResponsesEventError::Malformed("response.failed has no error".into())
                })?;
            Ok(ResponsesEvent::Failed {
                error: ResponseError {
                    code: error
                        .get("code")
                        .and_then(Value::as_str)
                        .unwrap_or("provider_failed")
                        .to_owned(),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("provider response failed")
                        .to_owned(),
                },
            })
        }
        "response.incomplete" => Ok(ResponsesEvent::Incomplete {
            reason: object
                .get("incomplete_details")
                .or_else(|| {
                    object
                        .get("response")
                        .and_then(|response| response.get("incomplete_details"))
                })
                .and_then(Value::as_object)
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str)
                .or_else(|| object.get("reason").and_then(Value::as_str))
                .unwrap_or("unknown")
                .to_owned(),
        }),
        other => Ok(ResponsesEvent::Unknown {
            r#type: other.to_owned(),
        }),
    }
    .map(|event| (event, matching_identity))
}

fn native_agent_id(value: &Value) -> Option<String> {
    value
        .pointer("/response/conversation/id")
        .and_then(Value::as_str)
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 256
                && id.trim() == *id
                && !id.chars().any(char::is_control)
        })
        .map(str::to_owned)
}

fn matches_turn_identity(
    object: &serde_json::Map<String, Value>,
    kind: &str,
    accepted_response: Option<&str>,
) -> bool {
    let response = object.get("response");
    let nested = response.and_then(|response| response.get("id"));
    // The legacy top-level id only supplies response identity without a nested id.
    let top = (nested.is_none()
        && matches!(
            kind,
            "response.created" | "response.in_progress" | "response.completed"
        ))
    .then(|| object.get("id"))
    .flatten();
    let accepted_response = accepted_response.or_else(|| {
        matches!(kind, "response.created" | "response.in_progress")
            .then(|| nested.or(top).and_then(Value::as_str))
            .flatten()
    });
    accepted_response.is_none_or(|accepted| {
        [
            object.get("response_id"),
            nested,
            response.and_then(|response| response.get("response_id")),
            top,
        ]
        .into_iter()
        .flatten()
        .all(|id| id.as_str() == Some(accepted))
    })
}

/// Recover the response identifier from an acceptance frame without changing
/// the compact `InProgress` event shape.
pub fn response_id_from_frame(frame: &SseFrame) -> Option<String> {
    serde_json::from_str::<Value>(&frame.data)
        .ok()
        .and_then(|value| {
            value
                .get("response")
                .and_then(Value::as_object)
                .and_then(|response| response.get("id"))
                .or_else(|| value.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

fn response_id(object: &serde_json::Map<String, Value>) -> Result<String, ResponsesEventError> {
    object
        .get("response")
        .and_then(Value::as_object)
        .and_then(|response| response.get("id"))
        .or_else(|| object.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ResponsesEventError::Malformed("response.created has no response id".into()))
}

fn index(object: &serde_json::Map<String, Value>) -> Result<usize, ResponsesEventError> {
    object
        .get("output_index")
        .or_else(|| object.get("index"))
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| ResponsesEventError::Malformed("response output event has no index".into()))
}

fn string_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<String, ResponsesEventError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ResponsesEventError::Malformed(format!("response event has no {field}")))
}

fn output_item(value: Option<&Value>) -> Result<OutputItem, ResponsesEventError> {
    let value =
        value.ok_or_else(|| ResponsesEventError::Malformed("output event has no item".into()))?;
    let object = value
        .as_object()
        .ok_or_else(|| ResponsesEventError::Malformed("output item is not an object".into()))?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("other");
    match kind {
        "message" => Ok(OutputItem::Message {
            id: string_in(object, "id"),
            text_so_far: object
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| object.get("content").and_then(Value::as_str))
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    object
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|part| {
                            part.get("type").and_then(Value::as_str) == Some("output_text")
                        })
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect()
                }),
        }),
        "mcp_call" => Ok(OutputItem::McpCall {
            id: string_in(object, "id"),
            name: string_in(object, "name"),
            status: string_in(object, "status"),
        }),
        "function_call" => Ok(OutputItem::FunctionCall {
            id: string_in(object, "id"),
            name: string_in(object, "name"),
            arguments: string_in(object, "arguments"),
        }),
        other => Ok(OutputItem::Other {
            r#type: other.to_owned(),
        }),
    }
}

fn string_in(object: &serde_json::Map<String, Value>, field: &str) -> String {
    object
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
#[path = "responses/trace_tests.rs"]
mod trace_tests;

#[cfg(test)]
mod tests;
