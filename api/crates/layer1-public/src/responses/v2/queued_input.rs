use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    sandbox::agent_io::{
        command_key_digest, CommandSubmissionSource, QueuedCancellationTarget,
        QueuedInputSubmission,
    },
    AgentId,
};
use serde::Deserialize;

use super::{error as response_error, ResponsesV2State};
use crate::AuthenticatedResourceCaller;

mod error;
#[cfg(test)]
mod pagination_tests;
mod shapes;
#[cfg(test)]
mod tests;
use shapes::{V2QueuedInput, V2QueuedInputPage, V2QueuedInputRequest};

type Auth = Result<AuthenticatedResourceCaller, crate::auth::NotAuthenticated>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NoQuery {}
type QueryResult = Result<Query<NoQuery>, axum::extract::rejection::QueryRejection>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListQuery {
    after: Option<u64>,
    limit: Option<usize>,
}
type ListQueryResult = Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>;

#[utoipa::path(get, path = "/api/v2/agents/{agent_id}/queued-inputs", tag = "response controls v2",
    params(("agent_id" = String, Path), ("after" = Option<u64>, Query, description = "Exclusive admission order cursor"), ("limit" = Option<usize>, Query, description = "Page size from 1 through 4; default 4")),
    responses((status = 200, body = V2QueuedInputPage), (status = 401, body = serde_json::Value),
        (status = 403, body = serde_json::Value), (status = 404, body = serde_json::Value), (status = 503, body = serde_json::Value)))]
pub(super) async fn list(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path(agent_id): Path<String>,
    query: ListQueryResult,
) -> Response {
    let Ok(auth) = auth else {
        return response_error::unauthenticated();
    };
    let Ok(Query(query)) = query else {
        return error::invalid();
    };
    let limit = query.limit.unwrap_or(4);
    if !(1..=4).contains(&limit)
        || query
            .after
            .is_some_and(|after| after == 0 || after > i64::MAX as u64)
    {
        return error::invalid();
    }
    match state
        .commands
        .list_queued_inputs(
            &auth.context,
            auth.credential
                .as_ref()
                .map(|credential| credential.value()),
            &AgentId(agent_id.clone()),
        )
        .await
    {
        Ok(data) => {
            let mut remaining = data
                .into_iter()
                .filter(|item| item.response.receipt.agent_order > query.after.unwrap_or(0));
            let data: Vec<shapes::V2QueuedInputItem> =
                remaining.by_ref().take(limit).map(Into::into).collect();
            let next_after = remaining
                .next()
                .and_then(|_| data.last().map(|item| item.response.receipt.agent_order));
            Json(V2QueuedInputPage {
                object: "cma.agent.queued_inputs",
                agent_id,
                data,
                next_after,
            })
            .into_response()
        }
        Err(value) => error::admission(value),
    }
}

#[utoipa::path(post, path = "/api/v2/agents/{agent_id}/responses/{response_id}/queued-input", tag = "response controls v2",
    request_body = V2QueuedInputRequest,
    params(("agent_id" = String, Path), ("response_id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = V2QueuedInput), (status = 400, body = serde_json::Value),
        (status = 401, body = serde_json::Value), (status = 403, body = serde_json::Value),
        (status = 404, body = serde_json::Value), (status = 409, body = serde_json::Value),
        (status = 422, body = serde_json::Value), (status = 503, body = serde_json::Value)))]
pub(super) async fn submit(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path((agent_id, response_id)): Path<(String, String)>,
    headers: HeaderMap,
    query: QueryResult,
    body: Result<Json<V2QueuedInputRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(auth) = auth else {
        return response_error::unauthenticated();
    };
    let Some(credential) = auth.credential.as_ref() else {
        return response_error::unauthenticated();
    };
    if query.is_err() {
        return error::invalid();
    }
    let body = match body {
        Ok(Json(body)) => body,
        Err(axum::extract::rejection::JsonRejection::JsonDataError(_)) => return error::invalid(),
        Err(_) => {
            return response_error::error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "A JSON queued input request is required.",
                Some("body"),
            )
        }
    };
    if headers.get_all("idempotency-key").iter().count() != 1 {
        return response_error::field(
            "Idempotency-Key",
            "invalid_idempotency_key",
            "Supply exactly one retained idempotency key.",
        );
    }
    let Some(key) = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|key| command_key_digest(key).is_ok())
    else {
        return response_error::field("Idempotency-Key", "invalid_idempotency_key", "Supply a retained idempotency key containing 1 to 256 bytes without control characters.");
    };
    let submission = QueuedInputSubmission {
        agent_id: AgentId(agent_id),
        target: QueuedCancellationTarget {
            response_id: AgentResponseId(response_id),
            source_command_id: AgentCommandId(body.source_command_id),
            expected_command_revision: body.expected_command_revision,
        },
        idempotency_key: key.to_owned(),
        text: body.text,
    };
    if submission.target.fingerprint(&submission.agent_id).is_err() {
        return error::invalid();
    }
    match state
        .commands
        .edit_queued_input(
            &auth.context,
            credential.value(),
            CommandSubmissionSource::Api,
            submission,
        )
        .await
    {
        Ok(value) => Json(V2QueuedInput::from(value)).into_response(),
        Err(value) => error::edit(value),
    }
}
