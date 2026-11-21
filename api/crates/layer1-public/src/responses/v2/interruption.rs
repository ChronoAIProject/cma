use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::sandbox::agent_io::{
    CommandSubmissionSource, InterruptingMessageStatusView, InterruptingMessageSubmission,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{controls, error, ResponsesV2State};
use crate::AuthenticatedResourceCaller;
use controls::shapes::{
    V2CommandStatus, V2ResponseControlCommand, V2ResponseControlRequest, V2ResponseControlTarget,
};

#[cfg(test)]
mod tests;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct V2InterruptRequest {
    pub turn_sequence: u64,
    pub expected_agent_revision: u64,
    pub text: String,
}

#[derive(Serialize, ToSchema)]
pub struct V2Interruption {
    pub object: &'static str,
    pub control: V2CommandStatus,
    /// A distinct response admitted atomically with the cancellation. It starts
    /// only after the active turn ends, ahead of existing ordinary queued input.
    pub response: V2CommandStatus,
    pub target: V2ResponseControlTarget,
    pub expected_agent_revision: u64,
}

impl From<InterruptingMessageStatusView> for V2Interruption {
    fn from(value: InterruptingMessageStatusView) -> Self {
        Self {
            object: "cma.response.interruption",
            control: value.control.into(),
            response: value.response.into(),
            target: value.target.into(),
            expected_agent_revision: value.expected_agent_revision,
        }
    }
}

type Auth = Result<AuthenticatedResourceCaller, crate::auth::NotAuthenticated>;
type QueryResult = Result<Query<controls::NoQuery>, axum::extract::rejection::QueryRejection>;

/// Retain the exact key and body after an uncertain reply. The cancellation and
/// new input are one atomic admission, and retrying never creates another turn.
#[utoipa::path(post, path = "/api/v2/agents/{agent_id}/responses/{response_id}/interrupt", tag = "response controls v2",
    request_body = V2InterruptRequest,
    params(("agent_id" = String, Path), ("response_id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 202, body = V2Interruption), (status = 400, body = serde_json::Value),
        (status = 401, body = serde_json::Value), (status = 403, body = serde_json::Value),
        (status = 404, body = serde_json::Value), (status = 409, body = serde_json::Value),
        (status = 422, body = serde_json::Value), (status = 429, body = serde_json::Value),
        (status = 503, body = serde_json::Value)))]
pub(super) async fn submit(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path((agent_id, response_id)): Path<(String, String)>,
    headers: HeaderMap,
    query: QueryResult,
    body: Result<Json<V2InterruptRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(auth) = auth else {
        return error::unauthenticated();
    };
    let Some(credential) = auth.credential.as_ref() else {
        return error::unauthenticated();
    };
    if query.is_err() {
        return error::field(
            "query",
            "controls_field_unsupported",
            "This control endpoint accepts no query parameters.",
        );
    }
    let body = match body {
        Ok(Json(body)) => body,
        Err(axum::extract::rejection::JsonRejection::JsonDataError(_)) => {
            return error::field(
                "body",
                "invalid_interrupt_request",
                "Supply only turn_sequence, expected_agent_revision and nonempty text.",
            )
        }
        Err(_) => {
            return error::error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "A JSON interruption request is required.",
                Some("body"),
            )
        }
    };
    if layer2_core::sandbox::agent_io::AgentInput::text(body.text.clone())
        .canonical()
        .is_err()
    {
        return error::field(
            "text",
            "invalid_interrupt_request",
            "Supply bounded nonempty text.",
        );
    }
    let control = match controls::parse::parse(
        agent_id,
        response_id,
        &headers,
        V2ResponseControlRequest {
            turn_sequence: body.turn_sequence,
            expected_agent_revision: body.expected_agent_revision,
            command: V2ResponseControlCommand::Cancel {},
        },
    ) {
        Ok(control) => control,
        Err(response) => return *response,
    };
    match state
        .commands
        .interrupt_with_message(
            &auth.context,
            credential.value(),
            CommandSubmissionSource::Api,
            InterruptingMessageSubmission {
                agent_id: control.agent_id,
                target: control.target,
                expected_agent_revision: control.expected_agent_revision,
                text: body.text,
                idempotency_key: control.idempotency_key,
            },
        )
        .await
    {
        Ok(value) => (StatusCode::ACCEPTED, Json(V2Interruption::from(value))).into_response(),
        Err(layer2_core::sandbox::agent_io::TargetedControlError::NotAdmitted) => error::error(
            StatusCode::CONFLICT, "response_interrupt_not_admitted",
            "The original interruption was not admitted and its Agent revision is permanently stale. Observe again before creating a new intent.", None,
        ),
        Err(layer2_core::sandbox::agent_io::TargetedControlError::CapabilityUnsupported) => error::error(
            StatusCode::UNPROCESSABLE_ENTITY, "capability_unsupported",
            "The observed Agent runtime does not support interruption.", None,
        ),
        Err(layer2_core::sandbox::agent_io::TargetedControlError::Admission(value)) => controls::error::admission(value),
    }
}
