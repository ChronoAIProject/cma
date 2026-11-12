//! Personal authenticated recovery commands. Native facts never leave this route.
use std::{future::Future, sync::Arc};

use axum::{
    extract::{DefaultBodyLimit, FromRequestParts, OptionalFromRequestParts, Path, State},
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::{
    resource_access::CredentialSource,
    sandbox::agent_capacity_recovery::{
        commands::CapacityRecoveryCommands,
        ports::{CommitOutcome, RecoveryError},
        state::{RecoveryRequest, RecoveryWork},
    },
    AgentId, Caller,
};
use serde_json::json;

use crate::rest::{problem_document, problem_response};
use crate::{AuthMethod, AuthenticatedCaller, AuthenticatedResourceCaller};

pub fn router(service: Arc<dyn CapacityRecoveryCommands>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/capacity-recovery", get(status))
        .route(
            "/api/v1/agents/{id}/capacity-recovery/status",
            post(request_status),
        )
        .route("/api/v1/agents/{id}/capacity-recovery/admit", post(admit))
        .route("/api/v1/agents/{id}/capacity-recovery/resume", post(resume))
        .with_state(service)
        .layer(DefaultBodyLimit::max(4096))
        .layer(axum::middleware::map_response(super::private_no_store))
}

struct RequestAuthority {
    caller: Caller,
    deadline: Option<tokio::time::Instant>,
}

impl<S: Send + Sync> FromRequestParts<S> for RequestAuthority {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let AuthenticatedCaller(caller) =
            <AuthenticatedCaller as FromRequestParts<S>>::from_request_parts(parts, state)
                .await
                .map_err(IntoResponse::into_response)?;
        let context =
            <AuthenticatedResourceCaller as OptionalFromRequestParts<S>>::from_request_parts(
                parts, state,
            )
            .await
            .map_err(IntoResponse::into_response)?;
        if parts.extensions.get::<AuthMethod>().is_some()
            || context.as_ref().is_some_and(|context| {
                matches!(
                    context.context.credential_source,
                    CredentialSource::Binding | CredentialSource::RequestBoundRead { .. }
                )
            })
        {
            return Err(error(RecoveryError::Unauthorized));
        }
        let deadline = context
            .as_ref()
            .and_then(|context| context.context.credential_deadline());
        if deadline.is_some_and(|deadline| deadline <= tokio::time::Instant::now()) {
            return Err(error(RecoveryError::Unauthorized));
        }
        Ok(Self { caller, deadline })
    }
}

impl RequestAuthority {
    async fn execute<T>(
        &self,
        future: impl Future<Output = Result<T, RecoveryError>>,
    ) -> Result<T, RecoveryError> {
        if let Some(deadline) = self.deadline {
            tokio::time::timeout_at(deadline, future)
                .await
                .map_err(|_| RecoveryError::Unavailable)?
        } else {
            future.await
        }
    }
}

type Service = Arc<dyn CapacityRecoveryCommands>;

#[utoipa::path(get, path = "/api/v1/agents/{id}/capacity-recovery", tag = "agents",
    params(("id" = String, Path)),
    responses((status = 200, body = serde_json::Value), (status = 400), (status = 401), (status = 404), (status = 409), (status = 503)))]
async fn status(
    State(service): State<Service>,
    authority: RequestAuthority,
    Path(id): Path<String>,
) -> Response {
    match authority
        .execute(service.status(&authority.caller, &AgentId(id), None))
        .await
    {
        Ok(status) => Json(status).into_response(),
        Err(err) => error(err),
    }
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/capacity-recovery/status", tag = "agents",
    params(("id" = String, Path)),
    request_body(content = serde_json::Value, description = "Unchanged original request: agent_id, creation_key, creation_fingerprint, failure_seq and requested_at. This request is an identity, never authority."),
    responses((status = 200, body = serde_json::Value), (status = 400), (status = 401), (status = 404), (status = 409), (status = 503)))]
async fn request_status(
    State(service): State<Service>,
    authority: RequestAuthority,
    Path(id): Path<String>,
    Json(request): Json<RecoveryRequest>,
) -> Response {
    if !valid_request(&id, &request) {
        return invalid_request();
    }
    match authority
        .execute(service.status(&authority.caller, &request.agent_id, Some(&request)))
        .await
    {
        Ok(status) => Json(status).into_response(),
        Err(err) => error(err),
    }
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/capacity-recovery/admit", tag = "agents",
    params(("id" = String, Path)),
    request_body(content = serde_json::Value, description = "Unchanged original request: agent_id, creation_key, creation_fingerprint, failure_seq and requested_at. This request is an identity, never authority."),
    responses((status = 200, body = serde_json::Value), (status = 400), (status = 401), (status = 404), (status = 409), (status = 503) , (status = 202, body = serde_json::Value, description = "Unknown commit; retry the unchanged original request")))]
async fn admit(
    State(service): State<Service>,
    authority: RequestAuthority,
    Path(id): Path<String>,
    Json(request): Json<RecoveryRequest>,
) -> Response {
    if !valid_request(&id, &request) {
        return invalid_request();
    }
    outcome(
        &request,
        authority
            .execute(service.admit(&authority.caller, &request))
            .await,
    )
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/capacity-recovery/resume", tag = "agents",
    params(("id" = String, Path)),
    request_body(content = serde_json::Value, description = "Unchanged original request: agent_id, creation_key, creation_fingerprint, failure_seq and requested_at. This request is an identity, never authority."),
    responses((status = 200, body = serde_json::Value), (status = 400), (status = 401), (status = 404), (status = 409), (status = 503) , (status = 202, body = serde_json::Value, description = "Unknown commit; retry the unchanged original request")))]
async fn resume(
    State(service): State<Service>,
    authority: RequestAuthority,
    Path(id): Path<String>,
    Json(request): Json<RecoveryRequest>,
) -> Response {
    if !valid_request(&id, &request) {
        return invalid_request();
    }
    // Accepted work has no route-level admission flag or call to admit.
    outcome(
        &request,
        authority
            .execute(service.resume(&authority.caller, &request))
            .await,
    )
}

fn valid_request(id: &str, request: &RecoveryRequest) -> bool {
    request.agent_id.as_str() == id && request.validate().is_ok()
}

fn invalid_request() -> Response {
    let status = StatusCode::BAD_REQUEST;
    problem_response(
        status,
        problem_document(
            status,
            "invalid-recovery-request",
            "Supply the unchanged original recovery request for this Agent.",
            "invalid_recovery_request",
        ),
    )
}

fn outcome(
    request: &RecoveryRequest,
    result: Result<CommitOutcome<RecoveryWork>, RecoveryError>,
) -> Response {
    match result {
        Ok(CommitOutcome::Recorded(work)) => {
            let Ok(key) = work.request.key() else {
                return error(RecoveryError::InvalidEvidence);
            };
            if work.request != *request || work.validate().is_err() {
                return error(RecoveryError::InvalidEvidence);
            }
            Json(json!({"outcome": "recorded", "operation_key": key,
                "phase": work.phase, "attempts": work.attempts,
                "retry_at": (work.phase == layer2_core::sandbox::agent_capacity_recovery::state::RecoveryPhase::Admitted).then_some(&work.ready_at), "completed_at": work.completed_at})).into_response()
        }
        Ok(CommitOutcome::Unknown) => {
            (StatusCode::ACCEPTED, Json(json!({"outcome": "unknown"}))).into_response()
        }
        Ok(CommitOutcome::KnownAbort) => (
            StatusCode::CONFLICT,
            Json(json!({"outcome": "known_abort"})),
        )
            .into_response(),
        Err(err) => error(err),
    }
}

fn error(error: RecoveryError) -> Response {
    let (status, code, detail) = match error {
        RecoveryError::Unauthorized => (
            StatusCode::NOT_FOUND,
            "capacity_recovery_unavailable",
            "Agent recovery is unavailable.",
        ),
        RecoveryError::Disabled => (
            StatusCode::CONFLICT,
            "capacity_recovery_disabled",
            "New capacity recovery admission is disabled.",
        ),
        RecoveryError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "capacity_recovery_unavailable",
            "Retry the unchanged original request to reconcile its outcome.",
        ),
        RecoveryError::Conflict
        | RecoveryError::InvalidEvidence
        | RecoveryError::Disposition(_) => (
            StatusCode::CONFLICT,
            "capacity_recovery_conflict",
            "Current recovery prerequisites do not permit this operation.",
        ),
    };
    problem_response(
        status,
        problem_document(status, "capacity-recovery", detail, code),
    )
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(status, request_status, admit, resume))]
struct RecoveryCommandsApi;

pub(super) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    RecoveryCommandsApi::openapi()
}

#[cfg(test)]
mod tests;
