//! Owner-visible recovery facts and explicit, proof-bound reopening.
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{sandbox::RetainedRecoveryError, SandboxId};
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};

use super::{instances::HierarchyState, instances_problem::HierarchyProblem};
use crate::AuthenticatedCaller;

pub(super) fn document() -> utoipa::openapi::OpenApi {
    #[derive(OpenApi)]
    #[openapi(
        paths(inspect, recover),
        components(schemas(RetainedRecoveryView, RecoverRetainedRequest, RetainedReopenView))
    )]
    struct AssessmentApi;
    AssessmentApi::openapi()
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RetainedRecoveryView {
    /// Fixed classification of the latest complete assessment; never dispatch authority.
    pub disposition: String,
    /// The verified recovery path can be offered to this owner.
    pub admission_enabled: bool,
}

#[utoipa::path(get, path = "/api/v1/sandboxes/{id}/runtime-recovery", tag = "sandboxes",
    params(("id" = String, Path)),
    responses((status = 200, body = RetainedRecoveryView),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)))]
pub(super) async fn inspect(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<RetainedRecoveryView>, HierarchyProblem> {
    let assessment = state
        .retained_recovery
        .as_ref()
        .ok_or(HierarchyProblem::Unavailable)?
        .assess_with_reopening(&caller, &SandboxId(id), state.retained_reopening.as_deref())
        .await
        .map_err(|error| match error {
            RetainedRecoveryError::Forbidden => HierarchyProblem::Forbidden,
            RetainedRecoveryError::NotFound => HierarchyProblem::SandboxNotFound,
            RetainedRecoveryError::Unavailable => HierarchyProblem::Unavailable,
        })?;
    Ok(Json(RetainedRecoveryView {
        disposition: assessment.disposition.as_str().into(),
        admission_enabled: assessment.admission_enabled,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoverRetainedRequest {
    pub expected_revision: u64,
    pub confirm_same_image_recovery: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RetainedReopenView {
    pub operation_id: String,
    pub revision: u64,
    pub accepted_at: String,
}

#[utoipa::path(post, path = "/api/v1/sandboxes/{id}/runtime-recovery", tag = "sandboxes",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = RecoverRetainedRequest,
    responses((status = 202, body = RetainedReopenView),
        (status = 400, body = crate::rest::ProblemDocument),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument),
        (status = 409, body = crate::rest::ProblemDocument),
        (status = 422, body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)))]
pub(super) async fn recover(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: super::idempotency::IdempotencyKey,
    body: Result<super::idempotency::RequestBody<RecoverRetainedRequest>, super::BodyProblem>,
) -> Response {
    use layer2_core::{sandbox::RetainedReopenError as E, Capability};
    if !caller.authorization.allows(Capability::UseOwnSandboxes) {
        return HierarchyProblem::Forbidden.into_response();
    }
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let body = match body.and_then(|body| body.into_parts().0) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let Some(service) = state.retained_reopening.as_ref() else {
        return HierarchyProblem::Unavailable.into_response();
    };
    match service
        .request(
            &caller,
            &SandboxId(id),
            body.expected_revision,
            &key,
            body.confirm_same_image_recovery,
        )
        .await
    {
        Ok(accepted) => (
            StatusCode::ACCEPTED,
            Json(RetainedReopenView {
                operation_id: accepted.operation_id,
                revision: accepted.revision,
                accepted_at: accepted.accepted_at,
            }),
        )
            .into_response(),
        Err(error) => match error {
            E::Forbidden => HierarchyProblem::Forbidden,
            E::NotFound => HierarchyProblem::SandboxNotFound,
            E::InvalidConsent => HierarchyProblem::Invalid("retained_recovery_consent_required"),
            E::Conflict => HierarchyProblem::Conflict("retained_recovery_authority_changed"),
            E::StaleRequest => HierarchyProblem::Conflict("retained_recovery_revision_passed"),
            E::Refused => HierarchyProblem::Conflict("retained_recovery_not_proved"),
            E::Unavailable => {
                HierarchyProblem::SandboxLifecycleUnavailable("retained_recovery_unavailable")
            }
        }
        .into_response(),
    }
}
