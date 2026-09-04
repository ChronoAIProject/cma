use std::collections::BTreeMap;

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{CoreError, Event, EventKind, Session, SessionId, StopReason};
use serde::Deserialize;

use crate::auth::AuthenticatedCaller;
use crate::rest::{Problem, QueryProblem, SessionsState};

use super::{
    field_refusal, message_id, not_found, response_id, InputTokenDetails, OutputMessage,
    OutputText, OutputTokenDetails, ResponseError, ResponseObject, ResponseStatus, ResponseUsage,
    DEFAULT_MODEL_NAME, OBJECT_RESPONSE,
};

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RetrieveQuery {
    stream: Option<bool>,
    starting_after: Option<u64>,
    include: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/responses/{response_id}",
    tag = "responses",
    params(("response_id" = String, Path, description = "Durable response identifier")),
    responses(
        (status = 200, description = "Response reconstructed from durable history", body = ResponseObject),
        (status = 404, description = "Unknown, foreign, non-chat, or non-turn response", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "Stream resume or optional includes are outside the declared subset", body = crate::rest::ProblemDocument, content_type = "application/problem+json")
    )
)]
pub(super) async fn retrieve(
    State(state): State<SessionsState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(response): Path<String>,
    query: Result<Query<RetrieveQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(error) => return QueryProblem(error).into_response(),
    };
    if query.stream.is_some() {
        return field_refusal(
            "stream",
            "Stream resume is not available; retrieve the durable response without stream parameters.",
        );
    }
    if query.starting_after.is_some() {
        return field_refusal(
            "starting_after",
            "Stream resume is not available; retrieve the durable response without stream parameters.",
        );
    }
    if query.include.is_some() {
        return field_refusal(
            "include",
            "The Responses chat subset has no optional response fields to include.",
        );
    }
    let Some((id, turn)) = parse_response_id(&response) else {
        return not_found();
    };
    let session = match state.sessions.get_session(&caller, &id).await {
        Ok(session) if session.chat.is_some() => session,
        Ok(_) => return not_found(),
        Err(CoreError::Forbidden | CoreError::SessionNotFound) => return not_found(),
        Err(error) => return Problem::from(error).into_response(),
    };
    let events = match state.sessions.events_since(&caller, &id, None).await {
        Ok(events) => events,
        Err(error) => return Problem::from(error).into_response(),
    };
    match project(&session, &events, turn) {
        Some(response) => Json(response).into_response(),
        None => not_found(),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/responses/{response_id}",
    tag = "responses",
    params(("response_id" = String, Path)),
    responses((status = 422, description = "Response deletion is deliberately absent", body = crate::rest::ProblemDocument, content_type = "application/problem+json"))
)]
pub(super) async fn delete(
    AuthenticatedCaller(_caller): AuthenticatedCaller,
    Path(_response): Path<String>,
) -> Response {
    field_refusal(
        "delete",
        "Response deletion is not available because responses are projections of the durable conversation log.",
    )
}

#[utoipa::path(
    post,
    path = "/api/v1/responses/{response_id}/cancel",
    tag = "responses",
    params(("response_id" = String, Path)),
    responses((status = 422, description = "Background response cancellation is deliberately absent", body = crate::rest::ProblemDocument, content_type = "application/problem+json"))
)]
pub(super) async fn cancel(
    AuthenticatedCaller(_caller): AuthenticatedCaller,
    Path(_response): Path<String>,
) -> Response {
    field_refusal(
        "background",
        "Background responses and response cancellation are not available in the chat subset.",
    )
}

pub(super) fn parse_response_id(value: &str) -> Option<(SessionId, u64)> {
    let value = value.strip_prefix("resp_")?;
    let (session, turn) = value.rsplit_once('_')?;
    if !session.starts_with("sesn_") {
        return None;
    }
    let turn = turn.parse::<u64>().ok().filter(|turn| *turn > 0)?;
    Some((SessionId(session.to_string()), turn))
}

pub(super) fn latest_turn(events: &[Event]) -> Option<u64> {
    events
        .iter()
        .rev()
        .find_map(|event| matches!(event.kind, EventKind::TurnStarted).then_some(event.seq))
}

pub(super) fn project(session: &Session, events: &[Event], turn: u64) -> Option<ResponseObject> {
    let start_index = events
        .iter()
        .position(|event| event.seq == turn && matches!(event.kind, EventKind::TurnStarted))?;
    let previous_turn = events[..start_index]
        .iter()
        .rev()
        .find_map(|event| matches!(event.kind, EventKind::TurnStarted).then_some(event.seq));
    let end_index = events[start_index + 1..]
        .iter()
        .position(|event| matches!(event.kind, EventKind::TurnStarted))
        .map(|offset| start_index + 1 + offset)
        .unwrap_or(events.len());
    let slice = &events[start_index..end_index];
    let completed = slice
        .iter()
        .any(|event| matches!(event.kind, EventKind::TurnCompleted));
    if end_index < events.len() && !completed {
        return None;
    }

    let mut metadata = BTreeMap::new();
    let mut output = None;
    let mut reasoning_summary = None;
    let mut usage = None;
    let mut failure = None;
    let mut incomplete = None;
    for event in slice {
        match &event.kind {
            EventKind::TurnMetadata { metadata: retained } => metadata = retained.clone(),
            EventKind::MessageCommitted { content } => output = Some(content.clone()),
            EventKind::ReasoningCommitted { content } => reasoning_summary = Some(content.clone()),
            EventKind::UsageReported {
                input_tokens,
                output_tokens,
                ..
            } => {
                usage = Some(ResponseUsage {
                    input_tokens: *input_tokens,
                    input_tokens_details: InputTokenDetails::default(),
                    output_tokens: *output_tokens,
                    output_tokens_details: OutputTokenDetails::default(),
                    total_tokens: input_tokens.saturating_add(*output_tokens),
                });
            }
            EventKind::StatusChanged {
                stop_reason: Some(StopReason::Error | StopReason::Revoked),
                detail,
                capability,
                ..
            } if completed => {
                failure = Some(ResponseError {
                    code: if capability.is_some() {
                        "capability_unsupported".into()
                    } else {
                        "turn_failed".into()
                    },
                    message: detail
                        .clone()
                        .unwrap_or_else(|| "The response turn failed.".into()),
                });
            }
            EventKind::StatusChanged {
                stop_reason: Some(StopReason::BudgetReached),
                ..
            } if completed => {
                incomplete = Some(serde_json::json!({ "reason": "max_output_tokens" }));
            }
            _ => {}
        }
    }

    let status = if failure.is_some() {
        ResponseStatus::Failed
    } else if incomplete.is_some() {
        ResponseStatus::Incomplete
    } else if completed {
        ResponseStatus::Completed
    } else {
        ResponseStatus::InProgress
    };
    let output = output
        .map(|text| OutputMessage {
            id: message_id(&session.id, turn),
            kind: "message",
            status: if completed {
                "completed"
            } else {
                "in_progress"
            },
            role: "assistant",
            content: vec![OutputText {
                kind: "output_text",
                text,
                annotations: Vec::new(),
                logprobs: Vec::new(),
            }],
        })
        .into_iter()
        .collect();
    Some(ResponseObject {
        id: response_id(&session.id, turn),
        object: OBJECT_RESPONSE,
        created_at: layer2_core::seconds_from_rfc3339(&events[start_index].appended_at)
            .unwrap_or_default(),
        status,
        background: false,
        error: failure,
        incomplete_details: incomplete,
        instructions: None,
        max_output_tokens: None,
        model: session
            .options
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_NAME.into()),
        output,
        parallel_tool_calls: false,
        previous_response_id: previous_turn.map(|previous| response_id(&session.id, previous)),
        reasoning: serde_json::json!({
            "effort": null,
            "summary": reasoning_summary.map(|text| vec![serde_json::json!({
                "type": "summary_text",
                "text": text
            })])
        }),
        store: true,
        temperature: None,
        text: serde_json::json!({
            "format": { "type": "text" },
            "verbosity": "medium"
        }),
        tool_choice: "none",
        tools: Vec::new(),
        top_p: None,
        truncation: "disabled",
        usage,
        metadata,
    })
}
