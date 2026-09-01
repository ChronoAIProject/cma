use std::{collections::BTreeMap, sync::Arc, time::Duration};

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{CoreError, SessionId};
use serde::Deserialize;
use serde_json::{Map, Value};
use utoipa::ToSchema;

use crate::auth::AuthenticatedCaller;
use crate::rest::{BodyProblem, Problem, SessionsState};

use super::retrieve::{latest_turn, parse_response_id};
use super::{field_refusal, not_found, previous_conflict};

const ADMISSION_BOUND: Duration = Duration::from_secs(10 * 60);

/// OpenAPI request shape. Runtime parsing is presence-aware so explicitly null
/// refused fields cannot disappear through `Option<T>`.
#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
pub(super) struct CreateResponseRequestSchema {
    conversation: String,
    input: Value,
    stream: bool,
    metadata: Option<BTreeMap<String, String>>,
    previous_response_id: Option<String>,
    model: Option<String>,
    store: Option<bool>,
}

#[derive(Debug)]
pub(super) struct ParsedRequest {
    pub(super) conversation: SessionId,
    pub(super) input: String,
    pub(super) metadata: BTreeMap<String, String>,
    pub(super) previous_response_id: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/responses",
    tag = "responses",
    request_body = CreateResponseRequestSchema,
    responses(
        (status = 200, description = "One response turn as Server-Sent Events", body = String, content_type = "text/event-stream"),
        (status = 404, description = "Unknown, foreign, or non-chat conversation", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "previous_response_id no longer names the conversation end", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "A field is outside the declared Responses chat subset", body = crate::rest::ProblemDocument, content_type = "application/problem+json")
    )
)]
pub(super) async fn create(
    State(state): State<SessionsState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => return BodyProblem::from(error).into_response(),
    };
    let request = match parse(body) {
        Ok(request) => request,
        Err(response) => return *response,
    };
    let session = match state
        .sessions
        .get_session(&caller, &request.conversation)
        .await
    {
        Ok(session) if session.chat.is_some() => session,
        Ok(_) => return not_found(),
        Err(CoreError::Forbidden | CoreError::SessionNotFound) => return not_found(),
        Err(error) => return Problem::from(error).into_response(),
    };
    let events = match state
        .sessions
        .events_since(&caller, &request.conversation, None)
        .await
    {
        Ok(events) => events,
        Err(error) => return Problem::from(error).into_response(),
    };
    let latest = latest_turn(&events);
    let expected = match request.previous_response_id.as_deref() {
        None if latest.is_none() => None,
        None => return previous_conflict(),
        Some(previous) => match parse_response_id(previous) {
            Some((id, turn)) if id == request.conversation && Some(turn) == latest => Some(turn),
            _ => return previous_conflict(),
        },
    };

    // Hold a receiver before delivery whenever the live entry already exists.
    // The owned task survives both a dropped request and a dropped SSE body.
    let watching = state
        .sessions
        .subscribe_live(&caller, &request.conversation)
        .await
        .ok();
    let service = Arc::clone(&state.sessions);
    let admission_service = Arc::clone(&state.sessions);
    let admission_caller = caller.clone();
    let admission_id = request.conversation.clone();
    let stream_id = request.conversation.clone();
    let input = request.input;
    let metadata = request.metadata;
    let mut admission = tokio::spawn(async move {
        let turn = admission_service
            .send_linear_prompt(&admission_caller, &admission_id, input, expected, metadata)
            .await?;
        let watching = match watching {
            Some(watching) => Some(watching),
            None => admission_service
                .subscribe_live(&admission_caller, &admission_id)
                .await
                .ok(),
        };
        Ok::<_, CoreError>((turn, watching))
    });
    let admitted = match tokio::time::timeout(ADMISSION_BOUND, &mut admission).await {
        Ok(Ok(Ok(admitted))) => admitted,
        Ok(Ok(Err(CoreError::InvalidState { .. }))) => return previous_conflict(),
        Ok(Ok(Err(error))) => return Problem::from(error).into_response(),
        Ok(Err(error)) => {
            tracing::error!(%error, "Responses prompt admission task failed");
            return Problem::from(CoreError::RuntimeUnavailable(
                "the response prompt admission task failed".into(),
            ))
            .into_response();
        }
        Err(_) => {
            // Dropping the JoinHandle detaches the already-owned prompt task.
            return super::stream::provisioning_timeout(stream_id, session);
        }
    };
    super::stream::open(service, caller, session, admitted.0, admitted.1)
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
        Some(Value::String(value)) if !value.trim().is_empty() => SessionId(value),
        _ => {
            return Err(refusal(
                "conversation",
                "conversation is required and must be a chat conversation identifier; stateless responses are unavailable.",
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
                "model, when supplied, must be a string; CMA accepts it but echoes the configured chat model.",
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
    if values.len() > layer2_core::MAX_TURN_METADATA_PAIRS {
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
        if key.chars().count() > layer2_core::MAX_TURN_METADATA_KEY_CHARS
            || value.chars().count() > layer2_core::MAX_TURN_METADATA_VALUE_CHARS
        {
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
