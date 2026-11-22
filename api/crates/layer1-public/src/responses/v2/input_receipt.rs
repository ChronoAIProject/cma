use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    resource_access::CredentialSource, sandbox::agent_io::CommandSubmissionSource, AgentId,
};
use serde_json::Value;

use super::{
    controls::shapes::V2CommandStatus, error, routes::CreateV2ResponseRequest, ResponsesV2State,
};
use crate::AuthenticatedResourceCaller;

/// A miss is indeterminate. The original command may have aged out of retention.
#[utoipa::path(post, path = "/api/v2/agents/{agent_id}/responses/receipt", tag = "responses v2",
    request_body = CreateV2ResponseRequest,
    params(("agent_id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = Option<V2CommandStatus>), (status = 400, body = Value),
        (status = 401, body = Value), (status = 403, body = Value), (status = 404, body = Value),
        (status = 409, body = Value), (status = 422, body = Value), (status = 503, body = Value)))]
pub(super) async fn lookup(
    State(state): State<ResponsesV2State>,
    auth: Result<AuthenticatedResourceCaller, crate::auth::NotAuthenticated>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(auth) = auth else {
        return error::unauthenticated();
    };
    let Some(credential) = auth.credential.as_ref() else {
        return error::unauthenticated();
    };
    if auth.context.credential_source == CredentialSource::Binding {
        return error::error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "This receipt is unavailable to this caller.",
            None,
        );
    }
    let mut keys = headers.get_all("idempotency-key").iter();
    let key = match (
        keys.next().and_then(|value| value.to_str().ok()),
        keys.next(),
    ) {
        (Some(key), None) => key.to_owned(),
        _ => {
            return error::field(
                "Idempotency-Key",
                "invalid_idempotency_key",
                "Supply the original idempotency key.",
            )
        }
    };
    let Ok(Json(body)) = body else {
        return error::error(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            "Supply the original response request.",
            Some("body"),
        );
    };
    if body.get("event_context").is_some() {
        return error::field(
            "event_context",
            "responses_field_unsupported",
            "Only Agent input receipts can be recovered here.",
        );
    }
    let submission = match super::parse::parse(body, Some(AgentId(agent_id)), key) {
        Ok(submission) => submission,
        Err(response) => return *response,
    };
    let result = state
        .commands
        .lookup_admitted(
            &auth.context,
            credential.value(),
            CommandSubmissionSource::Api,
            submission,
        )
        .await;
    match result {
        Ok(receipt) => Json(receipt.map(V2CommandStatus::from)).into_response(),
        Err(error) => error::admission(error),
    }
}
