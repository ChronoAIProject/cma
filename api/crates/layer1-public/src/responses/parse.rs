use super::field_refusal;
use axum::response::Response;
use layer2_core::AgentId;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub(super) struct ParsedRequest {
    pub conversation: AgentId,
    pub input: String,
    pub metadata: BTreeMap<String, String>,
    pub previous_response_id: Option<String>,
}

pub(super) fn parse(body: Value) -> Result<ParsedRequest, Box<Response>> {
    let Value::Object(mut body) = body else {
        return Err(refusal("body", "A response request must be a JSON object."));
    };
    for field in ["instructions", "reasoning", "tools", "tool_choice"] {
        if body.contains_key(field) {
            return Err(refusal(
                field,
                &format!("The Responses chat subset does not accept '{field}'."),
            ));
        }
    }
    for field in ["background", "stream_options", "starting_after", "include"] {
        if body.contains_key(field) {
            return Err(refusal(
                field,
                "Background execution and stream resume are not available in the Responses chat subset.",
            ));
        }
    }
    for field in body.keys() {
        if !matches!(
            field.as_str(),
            "conversation"
                | "input"
                | "stream"
                | "metadata"
                | "previous_response_id"
                | "model"
                | "store"
        ) {
            return Err(refusal(
                field,
                &format!("The Responses chat subset does not recognise '{field}'."),
            ));
        }
    }

    let conversation = match body.remove("conversation") {
        Some(Value::String(value)) if !value.trim().is_empty() => AgentId(value),
        _ => {
            return Err(refusal(
                "conversation",
                "conversation is required and must be a Agent conversation identifier; stateless responses are unavailable.",
            ))
        }
    };
    let input = match body.remove("input") {
        Some(value) => parse_input(value)?,
        None => {
            return Err(refusal(
                "input",
                "input is required and must contain exactly one user message.",
            ))
        }
    };
    if body.remove("stream") != Some(Value::Bool(true)) {
        return Err(refusal(
            "stream",
            "stream: true is required; non-streaming responses are unavailable in v1.",
        ));
    }
    if let Some(value) = body.remove("model") {
        if !matches!(value, Value::String(_)) {
            return Err(refusal(
                "model",
                "model, when supplied, must be a string; CMA uses the Agent's configured conversation model.",
            ));
        }
    }
    if let Some(value) = body.remove("store") {
        if value != Value::Bool(true) {
            return Err(refusal(
                "store",
                "store: false is unavailable because chat responses are durable conversation projections.",
            ));
        }
    }
    let metadata = match body.remove("metadata") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(Value::Object(values)) => parse_metadata(values)?,
        Some(_) => {
            return Err(refusal(
                "metadata",
                "metadata must be an object whose keys and values are strings.",
            ))
        }
    };
    let previous_response_id = match body.remove("previous_response_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value),
        Some(_) => {
            return Err(refusal(
                "previous_response_id",
                "previous_response_id must be a response identifier or null.",
            ))
        }
    };
    Ok(ParsedRequest {
        conversation,
        input,
        metadata,
        previous_response_id,
    })
}

fn parse_input(value: Value) -> Result<String, Box<Response>> {
    match value {
        Value::String(text) if !text.is_empty() => Ok(text),
        Value::Array(mut items) if items.len() == 1 => {
            let Value::Object(mut item) = items.remove(0) else {
                return Err(input_refusal());
            };
            if item.remove("role") != Some(Value::String("user".into())) {
                return Err(input_refusal());
            }
            if let Some(kind) = item.remove("type") {
                if kind != Value::String("message".into()) {
                    return Err(input_refusal());
                }
            }
            if !item.is_empty() && item.keys().any(|key| key != "content") {
                return Err(input_refusal());
            }
            match item.remove("content") {
                Some(Value::String(text)) if !text.is_empty() => Ok(text),
                Some(Value::Array(mut content)) if content.len() == 1 => {
                    let Value::Object(mut part) = content.remove(0) else {
                        return Err(input_refusal());
                    };
                    if part.remove("type") != Some(Value::String("input_text".into())) {
                        return Err(input_refusal());
                    }
                    match part.remove("text") {
                        Some(Value::String(text)) if !text.is_empty() && part.is_empty() => {
                            Ok(text)
                        }
                        _ => Err(input_refusal()),
                    }
                }
                _ => Err(input_refusal()),
            }
        }
        _ => Err(input_refusal()),
    }
}

fn input_refusal() -> Box<Response> {
    refusal(
        "input",
        "input must be a non-empty string or an array containing exactly one user message with text content.",
    )
}

fn parse_metadata(values: Map<String, Value>) -> Result<BTreeMap<String, String>, Box<Response>> {
    if values.len() > 16 {
        return Err(refusal(
            "metadata",
            "metadata accepts at most 16 string pairs.",
        ));
    }
    let mut metadata = BTreeMap::new();
    for (key, value) in values {
        let Value::String(value) = value else {
            return Err(refusal(
                "metadata",
                "metadata keys and values must be strings.",
            ));
        };
        if key.chars().count() > 64 || value.chars().count() > 512 {
            return Err(refusal(
                "metadata",
                "metadata keys accept at most 64 characters and values at most 512 characters.",
            ));
        }
        metadata.insert(key, value);
    }
    Ok(metadata)
}

fn refusal(field: &str, detail: &str) -> Box<Response> {
    Box::new(field_refusal(field, detail))
}
