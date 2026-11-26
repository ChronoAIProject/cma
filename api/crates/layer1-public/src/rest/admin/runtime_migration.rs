//! Explicit operator migration and content-free forward observation.
use super::AdminState;
use crate::{
    rest::{instances_openapi_shapes::SandboxView, instances_problem::HierarchyProblem},
    AuthenticatedCaller,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use layer2_core::sandbox::{
    RuntimeMigrationCommand, RuntimeMigrationError, SandboxCustomRuntimeMigration,
};
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityTarget, Capability, DetailKey,
    SandboxId,
};
use serde::Deserialize;
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomRuntimeMigrationRequest {
    /// Exact immutable origin/home/profile and old image guards. No activity claims.
    #[schema(value_type = Object)]
    pub migration: SandboxCustomRuntimeMigration,
    pub authority_id: String,
    pub expected_current_revision: u64,
    #[serde(default)]
    pub restart_failed: bool,
}

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new()
        .route(
            "/api/v1/admin/sandboxes/{id}/runtime-migration",
            post(migrate),
        )
        .route(
            "/api/v1/admin/sandboxes/{id}/runtime-update/activity",
            get(observe),
        )
}

#[utoipa::path(post, path = "/api/v1/admin/sandboxes/{id}/runtime-migration", tag = "administration",
    params(("id" = String, Path)), request_body = CustomRuntimeMigrationRequest,
    responses((status = 202, body = SandboxView), (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument)))]
pub(crate) async fn migrate(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    body: Result<Json<CustomRuntimeMigrationRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<SandboxView>), HierarchyProblem> {
    if !caller.authorization.allows(Capability::ControlAnySandbox) {
        return Err(HierarchyProblem::Forbidden);
    }
    let Json(body) = body.map_err(|_| HierarchyProblem::Invalid("invalid_runtime_migration"))?;
    let service = state
        .runtime_migrations
        .as_ref()
        .ok_or(HierarchyProblem::Unavailable)?;
    let sandbox = service
        .request(
            &caller,
            &SandboxId(id),
            RuntimeMigrationCommand {
                migration: body.migration,
                authority_id: body.authority_id,
                expected_current_revision: body.expected_current_revision,
                restart_failed: body.restart_failed,
            },
        )
        .await
        .map_err(problem)?;
    let mut record = ActivityRecord::new(ActivityAction::Update, ActivityOutcome::Succeeded)
        .by(caller.subject)
        .on(ActivityTarget::Resource(format!("sandbox:{}", sandbox.id)));
    record
        .detail
        .record(DetailKey::Setting, "custom_runtime_migration");
    record
        .detail
        .record(DetailKey::Revision, sandbox.revision.to_string());
    state
        .activity
        .record(&record)
        .await
        .map_err(|_| HierarchyProblem::Unavailable)?;
    Ok((StatusCode::ACCEPTED, Json(SandboxView::from(&sandbox))))
}

#[utoipa::path(get, path = "/api/v1/admin/sandboxes/{id}/runtime-update/activity", tag = "administration",
    params(("id" = String, Path)), responses((status = 200, body = Object),
        (status = 403, body = crate::rest::ProblemDocument), (status = 503, body = crate::rest::ProblemDocument)))]
pub(crate) async fn observe(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<layer2_core::sandbox::RuntimeActivityStatus>, HierarchyProblem> {
    if !caller.authorization.allows(Capability::ReadAllSandboxes) {
        return Err(HierarchyProblem::Forbidden);
    }
    let status = if let Some(verifier) = &state.runtime_activity {
        verifier
            .observe(&SandboxId(id))
            .await
            .map_err(|_| HierarchyProblem::Unavailable)?
    } else {
        layer2_core::sandbox::RuntimeActivityStatus::Unknown {
            reason: layer2_core::sandbox::store::RuntimeActivityUnknownReason::UnqualifiedSource,
        }
    };
    // Like the existing admin Sandbox GET, observing does not append an operator
    // mutation audit. It never writes a maintenance/fence/repair document.
    Ok(Json(status))
}

fn problem(error: RuntimeMigrationError) -> HierarchyProblem {
    match error {
        RuntimeMigrationError::Forbidden => HierarchyProblem::Forbidden,
        RuntimeMigrationError::NotFound => HierarchyProblem::SandboxNotFound,
        RuntimeMigrationError::NotAdmitted => {
            HierarchyProblem::Conflict("runtime_migration_not_admitted")
        }
        RuntimeMigrationError::Unavailable => HierarchyProblem::Unavailable,
    }
}
