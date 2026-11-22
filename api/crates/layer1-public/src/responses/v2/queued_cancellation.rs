use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    sandbox::agent_io::{
        command_key_digest, CommandSubmissionSource, QueuedCancellationSubmission,
        QueuedCancellationTarget,
    },
    AgentId,
};
use serde::Deserialize;

use super::{error as response_error, ResponsesV2State};
use crate::AuthenticatedResourceCaller;

mod error;
mod shapes;
#[cfg(test)]
mod tests;
use shapes::{V2QueuedCancellation, V2QueuedCancellationObservation, V2QueuedCancellationRequest};

/// Internal routing signal after the authorized local handler identifies a
/// queued Trigger projection that needs a different Agent Manager.
#[derive(Clone)]
pub struct QueuedCancellationNeedsOwner;

type Auth = Result<AuthenticatedResourceCaller, crate::auth::NotAuthenticated>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NoQuery {}
type QueryResult = Result<Query<NoQuery>, axum::extract::rejection::QueryRejection>;

#[utoipa::path(get, path = "/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation", tag = "response controls v2",
    params(("agent_id" = String, Path), ("response_id" = String, Path)),
    responses((status = 200, body = V2QueuedCancellationObservation), (status = 401, body = serde_json::Value),
        (status = 403, body = serde_json::Value), (status = 404, body = serde_json::Value),
        (status = 422, body = serde_json::Value), (status = 503, body = serde_json::Value)))]
pub(super) async fn observe(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path((agent_id, response_id)): Path<(String, String)>,
    query: QueryResult,
) -> Response {
    let Ok(auth) = auth else {
        return response_error::unauthenticated();
    };
    if query.is_err() {
        return error::invalid();
    }
    match state
        .commands
        .observe_queued_cancellation(
            &auth.context,
            auth.credential
                .as_ref()
                .map(|credential| credential.value()),
            &AgentId(agent_id),
            &AgentResponseId(response_id),
        )
        .await
    {
        Ok(value) => Json(V2QueuedCancellationObservation::from(value)).into_response(),
        Err(value) => error::admission(value),
    }
}

#[utoipa::path(post, path = "/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation", tag = "response controls v2",
    request_body = V2QueuedCancellationRequest,
    params(("agent_id" = String, Path), ("response_id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = V2QueuedCancellation), (status = 400, body = serde_json::Value),
        (status = 401, body = serde_json::Value), (status = 403, body = serde_json::Value),
        (status = 404, body = serde_json::Value), (status = 409, body = serde_json::Value),
        (status = 422, body = serde_json::Value), (status = 503, body = serde_json::Value)))]
pub(super) async fn submit(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path((agent_id, response_id)): Path<(String, String)>,
    headers: HeaderMap,
    query: QueryResult,
    body: Result<Json<V2QueuedCancellationRequest>, axum::extract::rejection::JsonRejection>,
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
                "A JSON queued cancellation request is required.",
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
    let submission = QueuedCancellationSubmission {
        agent_id: AgentId(agent_id),
        target: QueuedCancellationTarget {
            response_id: AgentResponseId(response_id),
            source_command_id: AgentCommandId(body.source_command_id),
            expected_command_revision: body.expected_command_revision,
        },
        idempotency_key: key.to_owned(),
    };
    if submission.target.fingerprint(&submission.agent_id).is_err() {
        return error::invalid();
    }
    match state
        .commands
        .cancel_queued_response(
            &auth.context,
            credential.value(),
            CommandSubmissionSource::Api,
            submission,
        )
        .await
    {
        Ok(value) => Json(V2QueuedCancellation::from(value)).into_response(),
        Err(value) => error::cancellation(value),
    }
}
