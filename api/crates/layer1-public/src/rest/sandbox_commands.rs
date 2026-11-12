//! Safe explicit lifecycle receipts. The domain ledger retains idempotency and
//! authority; public responses contain neither custody references nor aggregates.
use super::{
    idempotency::{IdempotencyKey, RequestBody},
    resource_access_problem::ResourceProblem,
    BodyProblem,
};
use crate::AuthenticatedResourceCaller;
use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::sandbox::agent_io::CommandAuthorizationError;
use layer2_core::sandbox::control::{
    SandboxControlError, SandboxControlService, SandboxControlServiceError,
    SandboxControlSubmission,
};
use std::sync::Arc;
pub(super) mod shapes;
use shapes::*;

pub fn sandbox_commands_router(service: Arc<SandboxControlService>) -> Router {
    Router::new()
        .route("/api/v1/sandboxes/{id}/commands", post(create))
        .route("/api/v1/sandbox-commands/{id}", get(read))
        .with_state(service)
}
#[utoipa::path(post, path = "/api/v1/sandboxes/{id}/commands", tag = "sandboxes",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)), request_body = SandboxCommandRequest,
    responses((status = 202, description = "Durable receipt, including replay of a terminal command. Applied means a lifecycle transition was committed, not that runtime work finished.", body = SandboxCommandReceiptView),
    (status = 400, body = super::ProblemDocument), (status = 403, body = super::ProblemDocument), (status = 404, body = super::ProblemDocument), (status = 409, body = super::ProblemDocument), (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn create(
    State(service): State<Arc<SandboxControlService>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<SandboxCommandRequest>, BodyProblem>,
) -> Response {
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let body = match body.and_then(|body| body.into_parts().0) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let Some(credential) = caller.credential else {
        return problem(SandboxControlServiceError::Authority(
            CommandAuthorizationError::Revoked,
        ));
    };
    match service
        .admit(
            &caller.context,
            credential.value(),
            SandboxControlSubmission {
                sandbox_id: layer2_core::SandboxId(id),
                command: body.command.into(),
                expected_revision: body.expected_revision,
                idempotency_key: key,
            },
        )
        .await
    {
        Ok(record) => {
            let location = format!("/api/v1/sandbox-commands/{}", record.envelope.command_id);
            let mut response = (
                StatusCode::ACCEPTED,
                Json(SandboxCommandReceiptView::from(record)),
            )
                .into_response();
            response.headers_mut().insert(
                header::LOCATION,
                location.parse().expect("generated command identifier"),
            );
            private(response)
        }
        Err(error) => problem(error),
    }
}
#[utoipa::path(get, path = "/api/v1/sandbox-commands/{id}", tag = "sandboxes", params(("id" = String, Path)),
    responses((status = 200, body = SandboxCommandReceiptView), (status = 403, body = super::ProblemDocument), (status = 404, body = super::ProblemDocument), (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn read(
    State(service): State<Arc<SandboxControlService>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
) -> Response {
    let Some(credential) = caller.credential else {
        return problem(SandboxControlServiceError::Authority(
            CommandAuthorizationError::Revoked,
        ));
    };
    match service
        .read(
            &caller.context,
            credential.value(),
            &layer2_core::ids::SandboxCommandId(id),
        )
        .await
    {
        Ok(record) => private(Json(SandboxCommandReceiptView::from(record)).into_response()),
        Err(error) => problem(error),
    }
}
fn private(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}
fn problem(error: SandboxControlServiceError) -> Response {
    use SandboxControlServiceError::*;
    // A platform refusal lifts only by an operator revision: no Retry-After.
    let retryable = !matches!(error, Budget(_));
    let (status, code, detail) = match error {
        Budget(reason) => (super::instance_problem::budget_status(reason), reason.code(), reason.guidance()),
        Access(error) => return private(ResourceProblem(error).into_response()),
        Authority(CommandAuthorizationError::Revoked) => (StatusCode::FORBIDDEN, "command_authority_required", "Current credential custody does not authorize durable command execution."),
        Store(SandboxControlError::Invalid) => (StatusCode::BAD_REQUEST, "invalid_sandbox_command", "A Sandbox command and a positive expected revision are required."),
        Store(SandboxControlError::NotFound) => (StatusCode::NOT_FOUND, "sandbox_command_not_found", "Sandbox command not found."),
        Store(SandboxControlError::IdempotencyConflict) => (StatusCode::CONFLICT, "idempotency_key_reused", "This idempotency key names a different Sandbox command request."),
        Store(SandboxControlError::Conflict) => (StatusCode::CONFLICT, "sandbox_command_conflict", "Sandbox state or access changed. Read current state before submitting a new command."),
        Store(SandboxControlError::Busy) => (StatusCode::CONFLICT, "sandbox_command_busy", "Another lifecycle command is queued for this Sandbox."),
        Store(SandboxControlError::Corrupt | SandboxControlError::Unavailable) | Authority(CommandAuthorizationError::Unavailable) => (StatusCode::SERVICE_UNAVAILABLE, "sandbox_command_unavailable", "Sandbox commands are temporarily unavailable. Retry the same key to recover an unacknowledged receipt."),
    };
    let mut response = private(super::problem_response(
        status,
        super::problem_document(status, code, detail, code),
    ));
    if retryable && (status == StatusCode::SERVICE_UNAVAILABLE || code == "sandbox_command_busy") {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}
