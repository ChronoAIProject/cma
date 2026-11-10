//! Owner-visible read-only recovery facts, never a repair or admission endpoint.
use axum::{
    extract::{Path, State},
    Json,
};
use layer2_core::{sandbox::RetainedRecoveryError, SandboxId};
use serde::Serialize;
use utoipa::{OpenApi, ToSchema};

use super::{instances::HierarchyState, instances_problem::HierarchyProblem};
use crate::AuthenticatedCaller;

pub(super) fn document() -> utoipa::openapi::OpenApi {
    #[derive(OpenApi)]
    #[openapi(paths(inspect), components(schemas(RetainedRecoveryView)))]
    struct AssessmentApi;
    AssessmentApi::openapi()
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RetainedRecoveryView {
    /// Fixed classification of the latest complete assessment; never dispatch authority.
    pub disposition: String,
    /// False: this diagnostic slice does not schedule recovery or interrupt work.
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
        .assess(&caller, &SandboxId(id))
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
