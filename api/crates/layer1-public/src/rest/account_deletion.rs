//! Owner-only deletion acceptance and privacy-safe progress receipts.
use std::{collections::BTreeMap, sync::Arc};

use axum::{
    extract::{Extension, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::account_deletion::{
    AccountDeletionError, AccountDeletionId, AccountDeletionService, AccountDeletionStatus,
    ErasureStep, Receipt, StepProgress,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::idempotency::{IdempotencyKey, IdempotencyProblem, RequestBody};
use crate::{AuthMethod, AuthenticatedCaller};

pub fn account_deletion_router(service: Arc<AccountDeletionService>) -> Router {
    Router::new()
        .route("/api/v1/account/deletion", get(status).post(request))
        .with_state(service)
        .layer(axum::middleware::map_response(private_no_store))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct DeletionRequest {
    /// Explicit acknowledgement that deletion is irreversible.
    #[schema(example = "irreversible")]
    acknowledge: Option<String>,
}

#[derive(Serialize, ToSchema)]
struct DeletionReceipt {
    #[schema(value_type = String)]
    id: AccountDeletionId,
    #[schema(schema_with = status_schema)]
    status: AccountDeletionStatus,
    #[schema(schema_with = step_schema)]
    step: ErasureStep,
    requested_at: String,
    deadline_at: String,
    #[schema(required = true)]
    completed_at: Option<String>,
    steps: Vec<DeletionStepProgress>,
}
#[derive(Serialize, ToSchema)]
struct DeletionStepProgress {
    #[schema(schema_with = step_schema)]
    step: ErasureStep,
    completed_at: String,
    counts: BTreeMap<String, u64>,
}
fn status_schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
    utoipa::openapi::schema::ObjectBuilder::new()
        .schema_type(utoipa::openapi::schema::Type::String)
        .enum_values(Some(["running", "blocked", "completed"]))
        .into()
}
fn step_schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
    utoipa::openapi::schema::ObjectBuilder::new()
        .schema_type(utoipa::openapi::schema::Type::String)
        .enum_values(Some([
            "inventory",
            "retire_sandboxes",
            "release_homes",
            "erase_records",
            "erase_objects",
            "revoke_credentials",
            "verify_no_residue",
        ]))
        .into()
}
impl From<Receipt> for DeletionReceipt {
    fn from(value: Receipt) -> Self {
        Self {
            id: value.id,
            status: value.status,
            step: value.step,
            requested_at: value.requested_at,
            deadline_at: value.deadline_at,
            completed_at: value.completed_at,
            steps: value
                .steps
                .into_iter()
                .map(
                    |StepProgress {
                         step,
                         completed_at,
                         counts,
                     }| {
                        DeletionStepProgress {
                            step,
                            completed_at,
                            counts,
                        }
                    },
                )
                .collect(),
        }
    }
}

/// Accept irreversible deletion of the verified human caller's own CMA account.
#[utoipa::path(post, path = "/api/v1/account/deletion", tag = "account deletion",
    params(("Idempotency-Key" = String, Header, description = "Keep the original key on retries.")),
    request_body = DeletionRequest,
    responses((status = 202, body = DeletionReceipt,
        headers(("Location" = String), ("Cache-Control" = String))),
        (status = 400), (status = 401), (status = 403), (status = 409),
        (status = 415), (status = 422), (status = 503)))]
async fn request(
    State(service): State<Arc<AccountDeletionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    method: Option<Extension<AuthMethod>>,
    key: IdempotencyKey,
    body: RequestBody<DeletionRequest>,
) -> Response {
    if method.is_some() {
        return account_deletion_problem(AccountDeletionError::RequiresOwnerSession);
    }
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let body = match body.into_parts().0 {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    match service
        .request(
            &caller,
            &key,
            body.acknowledge.as_deref() == Some("irreversible"),
        )
        .await
    {
        Ok(receipt) => (
            StatusCode::ACCEPTED,
            [(header::LOCATION, "/api/v1/account/deletion")],
            Json(DeletionReceipt::from(receipt)),
        )
            .into_response(),
        Err(error) => account_deletion_problem(error),
    }
}

/// Read progress even when admission of new deletion requests is disabled.
#[utoipa::path(get, path = "/api/v1/account/deletion", tag = "account deletion",
    responses((status = 200, body = DeletionReceipt), (status = 401),
        (status = 403), (status = 404), (status = 503)))]
async fn status(
    State(service): State<Arc<AccountDeletionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    method: Option<Extension<AuthMethod>>,
) -> Response {
    if method.is_some() {
        return account_deletion_problem(AccountDeletionError::RequiresOwnerSession);
    }
    match service.status(&caller).await {
        Ok(receipt) => Json(DeletionReceipt::from(receipt)).into_response(),
        Err(error) => account_deletion_problem(error),
    }
}

/// Project account deletion refusals consistently at authentication boundaries.
pub fn account_deletion_problem(error: AccountDeletionError) -> Response {
    use AccountDeletionError::*;
    let (status, slug, code, detail) = match error {
        Conflict => return IdempotencyProblem::Reused.into_response(),
        RecoveryRequired => return IdempotencyProblem::RecoveryRequired.into_response(),
        Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "account-deletion-unavailable", "account_deletion_unavailable", "Account deletion is not enabled."),
        AcknowledgementRequired => (StatusCode::UNPROCESSABLE_ENTITY, "account-deletion-acknowledgement-required", "account_deletion_acknowledgement_required", "Acknowledge irreversible deletion before requesting it."),
        RequiresOwnerSession => (StatusCode::FORBIDDEN, "account-deletion-requires-owner-session", "account_deletion_requires_owner_session", "Account deletion requires a verified human owner session."),
        NotFound => (StatusCode::NOT_FOUND, "account-deletion-not-found", "account_deletion_not_found", "No account deletion request exists."),
        InProgress => (StatusCode::FORBIDDEN, "account-deletion-in-progress", "account_deletion_in_progress", "Account deletion is in progress."),
        Invalid => return IdempotencyProblem::Invalid.into_response(),
        Store(_) | StaleFence => (StatusCode::SERVICE_UNAVAILABLE, "store-failed", "store_failed", "Account deletion state is temporarily unavailable. Retry with the original Idempotency-Key."),
    };
    super::problem_response(status, super::problem_document(status, slug, detail, code))
}

// Include extractor refusals in the privacy policy.
async fn private_no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(request, status),
    components(schemas(DeletionRequest, DeletionReceipt, DeletionStepProgress))
)]
struct AccountDeletionApi;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    AccountDeletionApi::openapi()
}
