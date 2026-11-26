use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    sandbox::{Agent, AgentLogEntry, AgentLogKind},
    AgentId,
};
use serde::Deserialize;
use serde_json::json;

use super::{
    field_refusal, message_id, not_found, response_id, InputTokenDetails, OutputMessage,
    OutputText, OutputTokenDetails, ResponseError, ResponseObject, ResponseStatus, ResponseUsage,
};
use crate::{conversation::read_failure, AgentConversationState, AuthenticatedCaller};

#[path = "legacy_indexed.rs"]
pub(super) mod indexed;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RetrieveQuery {
    stream: Option<bool>,
    starting_after: Option<u64>,
    include: Option<String>,
    output_page: Option<bool>,
    output_after: Option<u64>,
}

#[utoipa::path(get, path = "/api/v1/responses/{response_id}", tag = "responses",
    params(("response_id" = String, Path)), responses((status = 200, body = ResponseObject),
    (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn retrieve(
    State(state): State<AgentConversationState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(response): Path<String>,
    resource: Option<crate::AuthenticatedResourceCaller>,
    query: Result<Query<RetrieveQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(_) => return field_refusal("query", "The retrieve query is invalid."),
    };
    if query.stream.is_some() || query.starting_after.is_some() || query.include.is_some() {
        return field_refusal("query", "Response stream resume and optional includes are unavailable; retrieve the durable response without query parameters.");
    }
    let Some((id, turn)) = parse_response_id(&response) else {
        return not_found();
    };
    if query.output_after.is_some() && query.output_page != Some(true) {
        return field_refusal(
            "output_after",
            "Use output_page=true with a returned output cursor.",
        );
    }
    let agent = match state.reads.read(&caller, &id).await {
        Ok(agent) => agent,
        Err(error) => return read_failure(error, "response_not_found"),
    };
    let origin = state.agent_page_origin.clone();
    let paged = query.output_page == Some(true);
    let from = query.output_after.unwrap_or(0);
    let held = if let Some(resource) = resource {
        if resource.context.caller != caller {
            return not_found();
        }
        Some(
            match state
                .reads
                .authorize_event_stream(
                    &resource.context,
                    resource.credential.as_ref().map(|c| c.value()),
                    &id,
                )
                .await
            {
                Ok(held) => held,
                Err(error) => return read_failure(error, "response_not_found"),
            },
        )
    } else {
        None
    };
    let result = if let Some(held) = &held {
        state
            .reads
            .with_output_observation(held, Some(turn), move |source| {
                Box::pin(async move {
                    indexed::retrieve_value(source, &origin, &agent, turn, paged, from).await
                })
            })
            .await
    } else {
        state
            .reads
            .with_owned_output_observation(&caller, &id, turn, move |source| {
                Box::pin(async move {
                    indexed::retrieve_value(source, &origin, &agent, turn, paged, from).await
                })
            })
            .await
    };
    match result {
        Ok(Some(Some(value)))=>return Json(value).into_response(),
        Ok(Some(None))=>return (axum::http::StatusCode::UNPROCESSABLE_ENTITY,Json(json!({"error":{"code":"response_pagination_required","message":"Read this response with output_page=true and its returned cursor; output was not truncated."}}))).into_response(),
        Err(error)=>return read_failure(error,"response_not_found"),
        Ok(None)=>{},
    }
    let (agent, history) = if let Some(held) = &held {
        let snapshot = match state.reads.event_stream_snapshot(held).await {
            Ok(snapshot) => snapshot,
            Err(error) => return read_failure(error, "response_not_found"),
        };
        let mut agent = match state.reads.read(&caller, &id).await {
            Ok(agent) => agent,
            Err(error) => return read_failure(error, "response_not_found"),
        };
        agent.profile.content.model = snapshot.initial_model;
        agent.conversation.last_seq = snapshot.head;
        let history = snapshot
            .history
            .into_iter()
            .filter_map(|row| {
                Some(AgentLogEntry {
                    agent_id: id.clone(),
                    seq: row.seq,
                    appended_at: row.appended_at,
                    kind: row.kind?,
                })
            })
            .collect::<Vec<_>>();
        (agent, history)
    } else {
        match state.reads.log_snapshot(&caller, &id).await {
            Ok(snapshot) => snapshot,
            Err(error) => return read_failure(error, "response_not_found"),
        }
    };
    let response = project(&state.agent_page_origin, &agent, &history, turn);
    if let Some(held) = &held {
        if let Err(error) = state.reads.validate_event_stream(held).await {
            return read_failure(error, "response_not_found");
        }
    }
    match response {
        Some(response) if paged => {
            let Ok(from) = usize::try_from(from) else {
                return field_refusal("output_after", "The output cursor is invalid.");
            };
            if from > response.output.len() {
                return field_refusal("output_after", "The output cursor is beyond the response.");
            }
            let next = response.output.len().min(from.saturating_add(1));
            if serde_json::to_vec(&response.output[from..next]).map_or(true, |b| {
                b.len() + 2048 > layer2_core::sandbox::store::indexed::OUTPUT_PAGE_BYTES
            }) {
                return field_refusal("output_page", "One output item exceeds the page budget.");
            }
            Json(json!({"object":"cma.response.output_page","response_id":response.id,"data":&response.output[from..next],"has_more":next<response.output.len(),"next_after":(next<response.output.len()).then_some(next)})).into_response()
        }
        Some(response) => Json(response).into_response(),
        None => not_found(),
    }
}

#[utoipa::path(delete, path = "/api/v1/responses/{response_id}", tag = "responses",
    params(("response_id" = String, Path)), responses((status = 422, body = crate::rest::ProblemDocument)))]
pub(super) async fn delete(AuthenticatedCaller(_caller): AuthenticatedCaller) -> Response {
    field_refusal(
        "delete",
        "Responses are projections of durable Agent history and cannot be deleted separately.",
    )
}

#[utoipa::path(post, path = "/api/v1/responses/{response_id}/cancel", tag = "responses",
    params(("response_id" = String, Path)), responses((status = 422, body = crate::rest::ProblemDocument)))]
pub(super) async fn cancel(AuthenticatedCaller(_caller): AuthenticatedCaller) -> Response {
    field_refusal(
        "background",
        "Background responses and response cancellation are outside the supported subset.",
    )
}

pub fn parse_response_id(value: &str) -> Option<(AgentId, u64)> {
    let (agent, turn) = value.strip_prefix("resp_")?.rsplit_once('_')?;
    if !agent.starts_with("agt_") || agent.len() <= 4 {
        return None;
    }
    let turn = turn.parse::<u64>().ok().filter(|turn| *turn > 0)?;
    if response_id(&AgentId(agent.into()), turn) != value {
        return None;
    }
    Some((AgentId(agent.into()), turn))
}

pub(super) fn project(
    origin: &crate::agent_page::AgentPageOrigin,
    agent: &Agent,
    history: &[AgentLogEntry],
    turn: u64,
) -> Option<ResponseObject> {
    let start = history.iter().position(|entry| entry.seq == turn
        && matches!(entry.kind, AgentLogKind::TurnStarted { turn_sequence, .. } if turn_sequence == turn))?;
    let previous = history[..start]
        .iter()
        .rev()
        .find_map(|entry| match entry.kind {
            AgentLogKind::TurnStarted { turn_sequence, .. } => Some(turn_sequence),
            _ => None,
        });
    let end = history[start + 1..]
        .iter()
        .position(|entry| matches!(entry.kind, AgentLogKind::TurnStarted { .. }))
        .map_or(history.len(), |offset| start + 1 + offset);
    let mut response = empty(origin, agent, turn);
    response.model = super::model::for_turn(
        agent.profile.content.model.as_deref(),
        turn,
        history.iter().map(|entry| (entry.seq, Some(&entry.kind))),
    )?;
    response.created_at =
        layer2_core::seconds_from_rfc3339(&history[start].appended_at).unwrap_or_default();
    response.previous_response_id = previous.map(|turn| response_id(&agent.id, turn));
    let mut lost_frames = 0_u64;
    let mut turn_active = true;
    for entry in &history[start..end] {
        match &entry.kind {
            AgentLogKind::AgentMessage { content } => {
                response
                    .output
                    .push(output_message(agent, turn, response.output.len(), content))
            }
            AgentLogKind::TurnMetadataSet { metadata, .. } => response.metadata = metadata.clone(),
            AgentLogKind::Usage {
                input_tokens,
                output_tokens,
                ..
            } => {
                response.usage = Some(ResponseUsage {
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    input_tokens_details: InputTokenDetails::default(),
                    output_tokens_details: OutputTokenDetails::default(),
                    total_tokens: input_tokens.saturating_add(*output_tokens),
                })
            }
            AgentLogKind::UsageUpdated {
                turn_sequence: Some(observed_turn),
                turn_input_tokens,
                turn_output_tokens,
                ..
            } if *observed_turn == turn => {
                response.usage = Some(ResponseUsage {
                    input_tokens: *turn_input_tokens,
                    output_tokens: *turn_output_tokens,
                    input_tokens_details: InputTokenDetails::default(),
                    output_tokens_details: OutputTokenDetails::default(),
                    total_tokens: turn_input_tokens.saturating_add(*turn_output_tokens),
                });
            }
            AgentLogKind::ConversationGap { frames } if turn_active => {
                lost_frames = lost_frames.saturating_add(*frames);
            }
            AgentLogKind::TurnCompleted { turn_sequence } if *turn_sequence == turn => {
                turn_active = false;
                response.status = ResponseStatus::Completed;
            }
            AgentLogKind::TurnFailed {
                turn_sequence,
                failure,
            } if *turn_sequence == turn => {
                turn_active = false;
                response.status = ResponseStatus::Failed;
                let failure = layer2_core::sandbox::public_agent_turn_failure(failure);
                response.error = Some(ResponseError {
                    code: failure.class,
                    message: failure.detail,
                });
            }
            AgentLogKind::TurnInterrupted { turn_sequence } if *turn_sequence == turn => {
                turn_active = false;
                response.status = ResponseStatus::Incomplete;
                response.incomplete_details = Some(json!({"reason": "interrupted"}));
            }
            _ => {}
        }
    }
    if end < history.len() && response.status == ResponseStatus::InProgress {
        return None;
    }
    if lost_frames > 0 && !turn_active {
        response.status = ResponseStatus::Incomplete;
        response.incomplete_details =
            Some(json!({"reason": "conversation_output_gap", "frames": lost_frames}));
    }
    Some(response)
}

pub(super) fn output_message(agent: &Agent, turn: u64, index: usize, text: &str) -> OutputMessage {
    OutputMessage {
        id: output_id(&agent.id, turn, index),
        kind: "message",
        status: "completed",
        role: "assistant",
        content: vec![OutputText {
            kind: "output_text",
            text: text.into(),
            annotations: Vec::new(),
            logprobs: Vec::new(),
        }],
    }
}

pub(super) fn output_id(agent: &AgentId, turn: u64, index: usize) -> String {
    if index == 0 {
        message_id(agent, turn)
    } else {
        format!("{}_{index}", message_id(agent, turn))
    }
}

pub(super) fn empty(
    origin: &crate::agent_page::AgentPageOrigin,
    agent: &Agent,
    turn: u64,
) -> ResponseObject {
    ResponseObject {
        callback_url: origin.agent_url(agent.id.as_str()),
        id: response_id(&agent.id, turn),
        object: "response",
        created_at: 0,
        status: ResponseStatus::InProgress,
        background: false,
        error: None,
        incomplete_details: None,
        instructions: None,
        max_output_tokens: None,
        model: super::model::named_or_default(agent.selected_model().as_deref()),
        output: Vec::new(),
        parallel_tool_calls: false,
        previous_response_id: None,
        reasoning: json!({"effort": null, "summary": null}),
        store: true,
        temperature: None,
        text: json!({"format": {"type": "text"}, "verbosity": "medium"}),
        tool_choice: "none",
        tools: Vec::new(),
        top_p: None,
        truncation: "disabled",
        usage: None,
        metadata: Default::default(),
    }
}
