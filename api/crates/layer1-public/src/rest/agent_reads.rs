//! Current-policy Agent history, usage and Sandbox resource projections.
use super::agent_openapi_shapes::{
    AgentEventPageView, AgentEventView, AgentResourcesView, AgentTranscriptMessageView,
    AgentTranscriptRoleView, AgentTranscriptTurnView, AgentTranscriptView, AgentUsageView,
};
use super::{agents::AgentRoutesState, instance_problem::InstanceProblem};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::Response,
    routing::get,
    Json, Router,
};
use layer2_core::{
    sandbox::{
        AgentLogKind, AgentResourceReading, AgentTranscript, AgentTranscriptRole,
        AgentUsageReading, AgentVisibleLogEntry,
    },
    AgentId,
};
use serde::Deserialize;
const DEFAULT_EVENT_LIMIT: u16 = 200;
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TranscriptParams {
    turn: Option<u64>,
    after: Option<u64>,
    through: Option<u64>,
    limit: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventParams {
    after: Option<u64>,
    limit: Option<u16>,
    #[serde(default)]
    tool_output: bool,
}

pub(super) fn routes() -> Router<AgentRoutesState> {
    Router::new()
        .route("/api/v1/agents/{id}/events", get(events))
        .route("/api/v1/agents/{id}/transcript", get(transcript))
        .route("/api/v1/agents/{id}/history", get(history))
        .route("/api/v1/agents/{id}/usage", get(usage))
        .route("/api/v1/agents/{id}/resources", get(resources))
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
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    params: Result<Query<EventParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<AgentEventPageView>, InstanceProblem> {
    let params = params.map_err(|_| InstanceProblem::InvalidQuery)?.0;
    let limit = params.limit.unwrap_or(DEFAULT_EVENT_LIMIT);
    if limit == 0 || limit > DEFAULT_EVENT_LIMIT {
        return Err(InstanceProblem::InvalidQuery);
    }
    let id = AgentId(id);
    let (entries, next) = if let Some(resource) = resource {
        state
            .reads
            .log_page_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &id,
                params.after.unwrap_or(0),
                limit,
            )
            .await?
    } else {
        let (entries, next) = state
            .reads
            .log_page(&caller, &id, params.after.unwrap_or(0), limit)
            .await?;
        let entries = entries
            .into_iter()
            .map(|entry| AgentVisibleLogEntry::from_entry(entry, true))
            .collect();
        (entries, next)
    };
    let items = entries
        .into_iter()
        .map(|mut entry| {
            if !params.tool_output {
                match &mut entry.kind {
                    Some(AgentLogKind::ToolOutputReported { .. }) => entry.kind = None,
                    Some(AgentLogKind::ToolCallUpdated { output, .. }) => *output = None,
                    _ => {}
                }
            }
            public_event(entry)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(AgentEventPageView { items, next }))
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}/transcript",
    tag = "agents",
    params(
        ("id" = String, Path),
        ("limit" = Option<u16>, Query, description = "Source rows per page (1..32). Omit all query parameters for complete legacy-compatible output."),
        ("turn" = Option<u64>, Query, description = "Continuation turn from next."),
        ("after" = Option<u64>, Query, description = "Continuation source sequence from next."),
        ("through" = Option<u64>, Query, description = "Fixed source high-water mark from next. Changed histories require a fresh first page.")
    ),
    responses((status = 200, body = AgentTranscriptView))
)]
pub(crate) async fn transcript(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    Query(params): Query<TranscriptParams>,
) -> Result<Json<AgentTranscriptView>, InstanceProblem> {
    let id = AgentId(id);
    if params.limit.is_some()
        || params.turn.is_some()
        || params.after.is_some()
        || params.through.is_some()
    {
        let request = layer2_core::sandbox::AgentTranscriptPageRequest {
            turn: params.turn,
            after: params.after,
            through: params.through,
            limit: params.limit.unwrap_or(32),
        };
        let page = if let Some(resource) = resource {
            state
                .reads
                .transcript_page_authorized(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                    &request,
                )
                .await
        } else {
            state.reads.transcript_page(&caller, &id, &request).await
        };
        let page = page.map_err(InstanceProblem::from)?;
        let mut value = public_transcript(page.transcript)?;
        value.next = page
            .next
            .map(|next| serde_json::to_value(next).map_err(|_| InstanceProblem::Unavailable))
            .transpose()?;
        return Ok(Json(value));
    }

    let value = if let Some(resource) = resource {
        state
            .reads
            .transcript_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &id,
            )
            .await
    } else {
        state.reads.transcript_bounded(&caller, &id).await
    };
    value
        .map_err(InstanceProblem::from)
        .and_then(public_transcript)
        .map(Json)
}

/// Download personal native history, retaining authority across the runtime IO.
pub(crate) async fn history(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Response, InstanceProblem> {
    let id = AgentId(id);
    let reading = if let Some(resource) = resource {
        state
            .reads
            .begin_history_read_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &id,
            )
            .await
    } else {
        state.reads.begin_history_read(&caller, &id).await
    }
    .map_err(InstanceProblem::from)?;
    let exporter = state
        .history_exporter
        .as_ref()
        .ok_or(InstanceProblem::Unavailable)?;
    let bytes = exporter
        .export(
            reading.runtime_id().map_err(InstanceProblem::from)?,
            id.as_str(),
        )
        .await;
    state
        .reads
        .finish_history_read(reading)
        .await
        .map_err(|error| {
            tracing::warn!(parent: None, stage = "authority_revalidation", reason = error.code(), "Agent raw history export withheld");
            InstanceProblem::from(error)
        })?;
    let bytes = bytes.map_err(InstanceProblem::HistoryExport)?;
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=agent-history.jsonl"),
    );
    Ok(response)
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
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Json<AgentUsageView>, InstanceProblem> {
    let id = AgentId(id);
    let value = if let Some(resource) = resource {
        state
            .reads
            .usage_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &id,
            )
            .await
    } else {
        state.reads.usage(&caller, &id).await
    };
    value
        .map_err(InstanceProblem::from)
        .map(public_usage)
        .map(Json)
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
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Json<AgentResourcesView>, InstanceProblem> {
    let id = AgentId(id);
    let value = if let Some(resource) = resource {
        state
            .reads
            .resources_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &id,
            )
            .await
    } else {
        state.reads.resources(&caller, &id).await
    };
    value
        .map_err(InstanceProblem::from)
        .map(public_resources)
        .map(Json)
}

fn public_event(entry: AgentVisibleLogEntry) -> Result<AgentEventView, InstanceProblem> {
    let Some(kind) = entry
        .kind
        .filter(|kind| !matches!(kind, AgentLogKind::AgentMessageDelta { .. }))
    else {
        return Ok(AgentEventView {
            retained_output: entry.retained_output.map(Into::into),
            seq: entry.seq,
            appended_at: entry.appended_at,
            kind: serde_json::json!({"type":"access_redacted"}),
        });
    };
    let kind = match kind {
        AgentLogKind::TurnStarted { turn_sequence, .. } => AgentLogKind::TurnStarted {
            model: None,
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
        AgentLogKind::TurnFailed {
            turn_sequence,
            failure,
        } => AgentLogKind::TurnFailed {
            turn_sequence,
            failure: layer2_core::sandbox::public_agent_turn_failure(&failure),
        },
        kind => kind,
    };
    Ok(AgentEventView {
        retained_output: entry.retained_output.map(Into::into),
        seq: entry.seq,
        appended_at: entry.appended_at,
        kind: serde_json::to_value(kind).map_err(|_| InstanceProblem::Unavailable)?,
    })
}

fn public_transcript(mut value: AgentTranscript) -> Result<AgentTranscriptView, InstanceProblem> {
    layer2_core::sandbox::sanitize_agent_transcript(&mut value);
    let turns = value
        .turns
        .into_iter()
        .map(|turn| {
            Ok::<_, InstanceProblem>(AgentTranscriptTurnView {
                retained_output: turn.retained_output.map(Into::into),
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
        next: None,
    })
}

fn public_usage(value: AgentUsageReading) -> AgentUsageView {
    AgentUsageView {
        agent_id: value.agent_id.to_string(),
        through_seq: value.through_seq,
        current_turn: value.usage.current_turn.map(Into::into),
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
#[path = "agent_failure_projection_tests.rs"]
mod provider_failure_tests;

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
