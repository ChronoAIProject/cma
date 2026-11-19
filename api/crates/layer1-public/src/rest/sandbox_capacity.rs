use std::collections::BTreeMap;

use axum::{extract::State, Json};
use serde::Serialize;
use utoipa::{OpenApi, ToSchema};

use super::{instances::HierarchyState, instances_problem::HierarchyProblem};
use crate::AuthenticatedCaller;

#[derive(Serialize, ToSchema)]
pub struct SandboxCapacityView {
    pub resource: &'static str,
    pub limit: u16,
    pub live: u64,
    pub available: u64,
    pub counts_by_status: BTreeMap<String, u64>,
    pub observed_at: String,
}

pub(super) fn document() -> utoipa::openapi::OpenApi {
    #[derive(OpenApi)]
    #[openapi(paths(read), components(schemas(SandboxCapacityView)))]
    struct CapacityApi;
    CapacityApi::openapi()
}

/// Current caller's Sandbox quota; this observation does not reserve capacity.
#[utoipa::path(get, path = "/api/v1/sandboxes/capacity", tag = "sandboxes",
    responses((status = 200, body = SandboxCapacityView),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<SandboxCapacityView>, HierarchyProblem> {
    let capacity = state
        .sandboxes
        .capacity(&caller)
        .await
        .map_err(HierarchyProblem::sandbox)?;
    Ok(Json(SandboxCapacityView {
        resource: "sandbox",
        limit: capacity.limit,
        live: capacity.live,
        available: capacity.available,
        counts_by_status: capacity.counts_by_status,
        observed_at: capacity.observed_at,
    }))
}
