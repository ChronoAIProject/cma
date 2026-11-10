use axum::response::Response;
use layer2_core::{
    sandbox::agent_io::{
        AgentCommand, AgentContent, AgentInput, AgentInputOrder, AnswerContext, CommandSubmission,
    },
    AgentId,
};
use serde_json::Value;
use std::collections::BTreeMap;

fn refusal(field: &str, code: &str, message: &str) -> Box<Response> {
    Box::new(super::error::field(field, code, message))
}
pub(super) fn parse(
    value: Value,
    alias: Option<AgentId>,
    key: String,
) -> Result<CommandSubmission, Box<Response>> {
    let Value::Object(mut body) = value else {
        return Err(refusal(
            "body",
            "invalid_response_request",
            "A response request must be a JSON object.",
        ));
    };
    if body.contains_key("previous_response_id")
        && body.get("previous_response_id") != Some(&Value::Null)
    {
        return Err(refusal("previous_response_id", if body.contains_key("conversation") || alias.is_some() { "conversation_previous_response_conflict" } else { "previous_response_unsupported" }, "Use conversation on every turn. previous_response_id continuation is unsupported and cannot be combined with conversation."));
    }
    for field in body.keys() {
        if !matches!(
            field.as_str(),
            "conversation"
                | "previous_response_id"
                | "input"
                | "stream"
                | "metadata"
                | "event_context"
                | "answer_context"
                | "model"
                | "store"
        ) {
            return Err(refusal(field, "responses_field_unsupported", "This field is outside the streaming Responses subset; execution configuration belongs to the pinned Agent Profile."));
        }
    }
    if body.remove("stream") != Some(Value::Bool(true)) {
        return Err(refusal(
            "stream",
            "stream_required",
            "Response creation requires stream: true.",
        ));
    }
    let conversation = match body.remove("conversation") {
        None => alias.clone().ok_or_else(|| {
            refusal(
                "conversation",
                "conversation_required",
                "conversation must name an existing Agent.",
            )
        })?,
        Some(Value::String(id)) if !id.trim().is_empty() => AgentId(id),
        Some(Value::Object(mut object)) if object.len() == 1 => match object.remove("id") {
            Some(Value::String(id)) if !id.trim().is_empty() => AgentId(id),
            _ => {
                return Err(refusal(
                    "conversation",
                    "invalid_conversation",
                    "conversation must be an Agent ID or an object containing its id.",
                ))
            }
        },
        _ => {
            return Err(refusal(
                "conversation",
                "invalid_conversation",
                "conversation must be an Agent ID or an object containing its id.",
            ))
        }
    };
    if alias.as_ref().is_some_and(|id| id != &conversation) {
        return Err(refusal(
            "conversation",
            "conversation_alias_mismatch",
            "conversation must match the Agent selected by the URL.",
        ));
    }
    let requested_model = match body.remove("model") {
        None => None,
        Some(Value::String(model)) if !model.trim().is_empty() => Some(model),
        _ => {
            return Err(refusal(
                "model",
                "invalid_model",
                "model must be a non-empty string when supplied.",
            ))
        }
    };
    if body
        .remove("store")
        .is_some_and(|value| value != Value::Bool(true))
    {
        return Err(refusal(
            "store",
            "store_required",
            "Responses are durable; only store: true is supported.",
        ));
    }
    let metadata: BTreeMap<String, String> = match body.remove("metadata") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(value) => serde_json::from_value(value).map_err(|_| {
            refusal(
                "metadata",
                "invalid_metadata",
                "metadata must contain string pairs.",
            )
        })?,
    };
    if metadata.len() > 16
        || metadata
            .iter()
            .any(|(key, value)| key.chars().count() > 64 || value.chars().count() > 512)
    {
        return Err(refusal(
            "metadata",
            "invalid_metadata",
            "metadata allows 16 pairs with at most 64 characters per key and 512 per value.",
        ));
    }
    let mut input = text_input(
        body.remove("input").unwrap_or(Value::Null),
        body.contains_key("event_context"),
    )?;
    if let Some(value) = body.remove("event_context") {
        input.content.push(AgentContent::ExternalEvent {
            context: super::event_context::parse(value)?,
        });
        if input
            .content
            .iter()
            .any(|item| matches!(item, AgentContent::Text { text } if !text.trim().is_empty()))
        {
            input = input.canonical().map_err(|_| {
                refusal(
                    "event_context",
                    "invalid_event_context",
                    "Input and event reference exceed the runtime input limit.",
                )
            })?;
        }
    }
    let mut command = AgentCommand::Message {
        input,
        order: AgentInputOrder::Append,
        metadata,
    };
    if let Some(value) = body.remove("answer_context") {
        let invalid = || {
            refusal(
                "answer_context",
                "invalid_answer_context",
                "Supply the exact durable turn_sequence and answer_sequence.",
            )
        };
        let context: AnswerContext = serde_json::from_value(value).map_err(|_| invalid())?;
        context.bind(&mut command).map_err(|_| invalid())?;
    }
    Ok(CommandSubmission {
        agent_id: conversation,
        command,
        idempotency_key: key,
        expected_agent_revision: None,
        requested_model,
    })
}
fn text_input(value: Value, allow_empty: bool) -> Result<AgentInput, Box<Response>> {
    let invalid = || {
        refusal("input", "invalid_response_input", "input must be non-empty text or exactly one user message containing text/input_text parts.")
    };
    let content = match value {
        Value::String(text) => vec![AgentContent::Text { text }],
        Value::Array(mut items) if items.len() == 1 => {
            let Value::Object(mut item) = items.remove(0) else {
                return Err(invalid());
            };
            if item.remove("role") != Some(Value::String("user".into()))
                || item.remove("type").is_some_and(|value| value != "message")
                || item.keys().any(|key| key != "content")
            {
                return Err(invalid());
            }
            match item.remove("content") {
                Some(Value::String(text)) => vec![AgentContent::Text { text }],
                Some(Value::Array(parts)) => {
                    let mut content = Vec::new();
                    for part in parts {
                        let Value::Object(mut part) = part else {
                            return Err(invalid());
                        };
                        if part.remove("type") != Some(Value::String("input_text".into())) {
                            return Err(invalid());
                        }
                        let Some(Value::String(text)) = part.remove("text") else {
                            return Err(invalid());
                        };
                        if !part.is_empty() {
                            return Err(invalid());
                        }
                        content.push(AgentContent::Text { text });
                    }
                    content
                }
                _ => return Err(invalid()),
            }
        }
        _ => return Err(invalid()),
    };
    if allow_empty
        && content.len() <= 31
        && content
            .iter()
            .all(|item| matches!(item, AgentContent::Text { text } if text.trim().is_empty()))
    {
        return Ok(AgentInput { content });
    }
    AgentInput { content }.canonical().map_err(|_| invalid())
}
