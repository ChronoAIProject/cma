//! Input, control, history, usage, and resource routes for one CMA Agent.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::sandbox::{
    AgentLogKind, AgentMessageOutcome, AgentResourceReading, AgentStopReason, AgentTranscript,
    AgentTranscriptRole, AgentUsageReading,
};
use layer2_core::AgentId;
use serde::Deserialize;

use super::agent_openapi_shapes::{
    AgentEventPageView, AgentEventView, AgentResourcesView, AgentTextRequest,
    AgentTranscriptMessageView, AgentTranscriptRoleView, AgentTranscriptTurnView,
    AgentTranscriptView, AgentUsageView, ApprovalDecisionRequest, InputAcceptedView,
    StopAgentRequest,
};
use super::agents::{execute_command, execute_empty_command, AgentRoutesState};
use super::idempotency::{operation_scope, IdempotencyKey, RequestBody};
use super::instance_problem::InstanceProblem;
use super::BodyProblem;
use crate::AuthenticatedCaller;

const DEFAULT_EVENT_LIMIT: u16 = 200;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventParams {
    after: Option<u64>,
    limit: Option<u16>,
}

pub(super) fn routes() -> Router<AgentRoutesState> {
    Router::new()
        .route("/api/v1/agents/{id}/input", post(input))
        .route("/api/v1/agents/{id}/steer", post(steer))
        .route("/api/v1/agents/{id}/interrupt", post(interrupt))
        .route("/api/v1/agents/{id}/approvals/{request_id}", post(approval))
        .route("/api/v1/agents/{id}/stop", post(stop))
        .route("/api/v1/agents/{id}/events", get(events))
        .route("/api/v1/agents/{id}/transcript", get(transcript))
        .route("/api/v1/agents/{id}/usage", get(usage))
        .route("/api/v1/agents/{id}/resources", get(resources))
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/input",
    tag = "agent operations",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = AgentTextRequest,
    responses(
        (status = 202, body = InputAcceptedView),
        (status = 409, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn input(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<AgentTextRequest>, BodyProblem>,
) -> Response {
    let service = state.agent_operations.clone();
    let operation_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("agent:input:{id}"),
        body,
        move |body| async move {
            let outcome = service
                .send_message(&operation_caller, &AgentId(id), body.text)
                .await
                .map_err(InstanceProblem::from)?;
            Ok((
                StatusCode::ACCEPTED,
                Json(InputAcceptedView {
                    queued: matches!(outcome, AgentMessageOutcome::Queued { .. }),
                }),
            )
                .into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/steer",
    tag = "agent operations",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = AgentTextRequest,
    responses((status = 202, description = "Steering accepted"))
)]
pub(crate) async fn steer(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<AgentTextRequest>, BodyProblem>,
) -> Response {
    let service = state.agent_operations.clone();
    let operation_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("agent:steer:{id}"),
        body,
        move |body| async move {
            service
                .steer(&operation_caller, &AgentId(id), body.text)
                .await
                .map_err(InstanceProblem::from)?;
            Ok(StatusCode::ACCEPTED.into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/interrupt",
    tag = "agent operations",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 202, description = "Interrupt accepted"))
)]
pub(crate) async fn interrupt(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    let service = state.agent_operations.clone();
    let operation_caller = caller.clone();
    execute_empty_command(
        &state,
        &caller,
        key,
        format!("agent:interrupt:{id}"),
        move || async move {
            service
                .interrupt(&operation_caller, &AgentId(id))
                .await
                .map_err(InstanceProblem::from)?;
            Ok(StatusCode::ACCEPTED.into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/approvals/{request_id}",
    tag = "agent operations",
    params(
        ("id" = String, Path),
        ("request_id" = String, Path),
        ("Idempotency-Key" = String, Header)
    ),
    request_body = ApprovalDecisionRequest,
    responses((status = 202, description = "Approval decision accepted"))
)]
pub(crate) async fn approval(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, request_id)): Path<(String, String)>,
    key: IdempotencyKey,
    body: Result<RequestBody<ApprovalDecisionRequest>, BodyProblem>,
) -> Response {
    let service = state.agent_operations.clone();
    let operation_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        key,
        operation_scope("agent:approval", &[&id, &request_id]),
        body,
        move |body| async move {
            service
                .answer_approval(
                    &operation_caller,
                    &AgentId(id),
                    request_id,
                    body.decision.into(),
                )
                .await
                .map_err(InstanceProblem::from)?;
            Ok(StatusCode::ACCEPTED.into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/stop",
    tag = "agents",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = StopAgentRequest,
    responses((status = 202, description = "Agent stop scheduled"))
)]
pub(crate) async fn stop(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<StopAgentRequest>, BodyProblem>,
) -> Response {
    let service = state.agents.clone();
    let operation_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("agent:stop:{id}"),
        body,
        move |body| async move {
            let _reason = body.reason;
            service
                .stop(&operation_caller, &AgentId(id), AgentStopReason::Owner)
                .await
                .map_err(InstanceProblem::from_stop)?;
            Ok(StatusCode::ACCEPTED.into_response())
        },
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}/events",
    tag = "agents",
    params(
        ("id" = String, Path),
        ("after" = Option<u64>, Query),
        ("limit" = Option<u16>, Query)
    ),
    responses((status = 200, body = AgentEventPageView))
)]
pub(crate) async fn events(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    params: Result<Query<EventParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<AgentEventPageView>, InstanceProblem> {
    let params = params.map_err(|_| InstanceProblem::InvalidQuery)?.0;
    let limit = params.limit.unwrap_or(DEFAULT_EVENT_LIMIT);
    if limit == 0 || limit > DEFAULT_EVENT_LIMIT {
        return Err(InstanceProblem::InvalidQuery);
    }
    let entries = state
        .reads
        .log(&caller, &AgentId(id), params.after.unwrap_or(0), limit)
        .await?;
    let next = (entries.len() == usize::from(limit))
        .then(|| entries.last().map(|entry| entry.seq))
        .flatten();
    let items = entries
        .into_iter()
        .map(public_event)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(AgentEventPageView { items, next }))
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}/transcript",
    tag = "agents",
    params(("id" = String, Path)),
    responses((status = 200, body = AgentTranscriptView))
)]
pub(crate) async fn transcript(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<AgentTranscriptView>, InstanceProblem> {
    state
        .reads
        .transcript(&caller, &AgentId(id))
        .await
        .map_err(InstanceProblem::from)
        .and_then(public_transcript)
        .map(Json)
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}/usage",
    tag = "agent usage",
    params(("id" = String, Path)),
    responses((status = 200, body = AgentUsageView))
)]
pub(crate) async fn usage(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<AgentUsageView>, InstanceProblem> {
    state
        .reads
        .usage(&caller, &AgentId(id))
        .await
        .map(public_usage)
        .map(Json)
        .map_err(InstanceProblem::from)
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}/resources",
    tag = "agent usage",
    params(("id" = String, Path)),
    responses((status = 200, body = AgentResourcesView))
)]
pub(crate) async fn resources(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<AgentResourcesView>, InstanceProblem> {
    state
        .reads
        .resources(&caller, &AgentId(id))
        .await
        .map(public_resources)
        .map(Json)
        .map_err(InstanceProblem::from)
}

fn public_event(
    entry: layer2_core::sandbox::AgentLogEntry,
) -> Result<AgentEventView, InstanceProblem> {
    let kind = match entry.kind {
        AgentLogKind::TurnStarted { turn_sequence, .. } => AgentLogKind::TurnStarted {
            turn_sequence,
            origin_key: None,
        },
        AgentLogKind::ApprovalRequested {
            approval_id,
            action,
            detail,
            ..
        } => AgentLogKind::ApprovalRequested {
            approval_id,
            action,
            detail,
            runtime_locator: None,
        },
        kind => kind,
    };
    Ok(AgentEventView {
        seq: entry.seq,
        appended_at: entry.appended_at,
        kind: serde_json::to_value(kind).map_err(|_| InstanceProblem::Unavailable)?,
    })
}

fn public_transcript(value: AgentTranscript) -> Result<AgentTranscriptView, InstanceProblem> {
    let turns = value
        .turns
        .into_iter()
        .map(|turn| {
            Ok::<_, InstanceProblem>(AgentTranscriptTurnView {
                turn_sequence: turn.turn_sequence,
                started_at: turn.started_at,
                owner_message: turn.owner_message,
                agent_messages: turn.agent_messages,
                messages: turn
                    .messages
                    .into_iter()
                    .map(|message| AgentTranscriptMessageView {
                        sequence: message.sequence,
                        role: match message.role {
                            AgentTranscriptRole::Owner => AgentTranscriptRoleView::User,
                            AgentTranscriptRole::Agent => AgentTranscriptRoleView::Assistant,
                        },
                        content: message.content,
                        appended_at: message.appended_at,
                    })
                    .collect(),
                outcome: serde_json::to_value(turn.outcome)
                    .map_err(|_| InstanceProblem::Unavailable)?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AgentTranscriptView {
        agent_id: value.agent_id.to_string(),
        through_seq: value.through_seq,
        turns,
    })
}

fn public_usage(value: AgentUsageReading) -> AgentUsageView {
    AgentUsageView {
        agent_id: value.agent_id.to_string(),
        through_seq: value.through_seq,
        input_tokens: value.usage.input_tokens,
        output_tokens: value.usage.output_tokens,
        context_used: value.usage.context_used,
        context_window: value.usage.context_window,
    }
}

fn public_resources(value: AgentResourceReading) -> AgentResourcesView {
    let reading = value.reading;
    AgentResourcesView {
        agent_id: value.agent_id.to_string(),
        sandbox_id: value.sandbox_id.to_string(),
        uptime_ms: reading.uptime_ms,
        cpu_usage_usec: reading.cpu_usage_usec,
        cpu_millis_limit: reading.cpu_millis_limit,
        memory_bytes_used: reading.memory_bytes_used,
        memory_bytes_limit: reading.memory_bytes_limit,
        memory_bytes_peak: reading.memory_bytes_peak,
        stall_permille: reading.stall_permille,
        ceiling_hits: reading.ceiling_hits,
        allocation_failures: reading.allocation_failures,
        kills: reading.kills,
        received_bytes: reading.received_bytes,
        sent_bytes: reading.sent_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transcript_preserves_ordered_steering_without_private_turn_metadata() {
        let at = "2026-09-07T00:00:00Z";
        let messages = json!([
            {"sequence":4,"role":"owner","content":"Review","appended_at":at},
            {"sequence":5,"role":"agent","content":"First answer","appended_at":at},
            {"sequence":7,"role":"owner","content":"Check the boundary","appended_at":at}
        ]);
        let transcript = serde_json::from_value(json!({
            "agent_id":"agt_steering","through_seq":7,"turns":[{
                "turn_sequence":3,"started_at":at,"origin_key":"private-origin",
                "metadata":{"private":"internal"},"owner_message":"Review",
                "agent_messages":["First answer"],"messages":messages,
                "outcome":{"type":"active"}
            }]
        }))
        .unwrap();
        let result = serde_json::to_value(
            public_transcript(transcript).unwrap_or_else(|_| panic!("valid transcript projection")),
        )
        .unwrap();
        let turn = &result["turns"][0];
        assert_eq!(
            turn["messages"],
            json!([
                {"sequence":4,"role":"user","content":"Review","appended_at":at},
                {"sequence":5,"role":"assistant","content":"First answer","appended_at":at},
                {"sequence":7,"role":"user","content":"Check the boundary","appended_at":at}
            ])
        );
        assert!(turn.get("origin_key").is_none());
        assert!(turn.get("metadata").is_none());
    }
}
