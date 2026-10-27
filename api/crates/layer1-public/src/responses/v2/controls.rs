use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    sandbox::agent_io::CommandSubmissionSource,
    AgentId,
};
use serde::Deserialize;

use super::{error as response_error, ResponsesV2State};
use crate::AuthenticatedResourceCaller;

pub(super) mod error;
#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod inspection_tests;
pub(super) mod parse;
pub(super) mod shapes;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod validation_tests;

use shapes::{V2CommandStatus, V2ResponseControl, V2ResponseControlRequest, V2ResponseControls};
type Auth = Result<AuthenticatedResourceCaller, crate::auth::NotAuthenticated>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NoQuery {}
type QueryResult = Result<Query<NoQuery>, axum::extract::rejection::QueryRejection>;

fn query_error() -> Response {
    response_error::field(
        "query",
        "controls_field_unsupported",
        "This control endpoint accepts no query parameters.",
    )
}

#[utoipa::path(get, path = "/api/v2/agents/{agent_id}/responses/{response_id}/controls", tag = "response controls v2",
    params(("agent_id" = String, Path), ("response_id" = String, Path)),
    responses((status = 200, body = V2ResponseControls), (status = 401, body = serde_json::Value), (status = 403, body = serde_json::Value),
        (status = 404, body = serde_json::Value), (status = 422, body = serde_json::Value), (status = 503, body = serde_json::Value)))]
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
        return query_error();
    }
    match state
        .commands
        .observe_response_controls(
            &auth.context,
            auth.credential
                .as_ref()
                .map(|credential| credential.value()),
            &AgentId(agent_id),
            &AgentResponseId(response_id),
        )
        .await
    {
        Ok(value) => Json(V2ResponseControls::from(value)).into_response(),
        Err(value) => error::admission(value),
    }
}

/// Preserve the exact key and body after a lost reply. A 202 accepts the command;
/// its later delivery does not prove that the response has been cancelled.
#[utoipa::path(post, path = "/api/v2/agents/{agent_id}/responses/{response_id}/controls", tag = "response controls v2",
    request_body = V2ResponseControlRequest,
    params(("agent_id" = String, Path), ("response_id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 202, body = V2ResponseControl), (status = 400, body = serde_json::Value), (status = 401, body = serde_json::Value),
        (status = 403, body = serde_json::Value), (status = 404, body = serde_json::Value), (status = 409, body = serde_json::Value),
        (status = 422, body = serde_json::Value), (status = 429, body = serde_json::Value), (status = 503, body = serde_json::Value)))]
pub(super) async fn submit(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path((agent_id, response_id)): Path<(String, String)>,
    headers: HeaderMap,
    query: QueryResult,
    body: Result<Json<V2ResponseControlRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(auth) = auth else {
        return response_error::unauthenticated();
    };
    let Some(credential) = auth.credential.as_ref() else {
        return response_error::unauthenticated();
    };
    if query.is_err() {
        return query_error();
    }
    let body = match body {
        Ok(Json(body)) => body,
        Err(axum::extract::rejection::JsonRejection::JsonDataError(_)) => {
            return response_error::field("body", "invalid_control_request",
                "Supply only turn_sequence, expected_agent_revision and a supported control command.");
        }
        Err(_) => {
            return response_error::error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "A JSON response control request is required.",
                Some("body"),
            )
        }
    };
    let submission = match parse::parse(agent_id, response_id, &headers, body) {
        Ok(value) => value,
        Err(value) => return *value,
    };
    match state
        .commands
        .admit_response_control(
            &auth.context,
            credential.value(),
            CommandSubmissionSource::Api,
            submission,
        )
        .await
    {
        Ok(value) => (StatusCode::ACCEPTED, Json(V2ResponseControl::from(value))).into_response(),
        Err(value) => error::control(value),
    }
}

#[utoipa::path(get, path = "/api/v2/commands/{command_id}", tag = "response controls v2",
    params(("command_id" = String, Path)), responses((status = 200, body = V2CommandStatus),
        (status = 401, body = serde_json::Value), (status = 403, body = serde_json::Value), (status = 404, body = serde_json::Value),
        (status = 422, body = serde_json::Value), (status = 503, body = serde_json::Value)))]
pub(super) async fn status(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path(command_id): Path<String>,
    query: QueryResult,
) -> Response {
    let Ok(auth) = auth else {
        return response_error::unauthenticated();
    };
    if query.is_err() {
        return query_error();
    }
    match state
        .commands
        .command_status(
            &auth.context,
            auth.credential
                .as_ref()
                .map(|credential| credential.value()),
            &AgentCommandId(command_id),
        )
        .await
    {
        Ok(value) => Json(V2CommandStatus::from(value)).into_response(),
        Err(value) => error::admission(value),
    }
}
