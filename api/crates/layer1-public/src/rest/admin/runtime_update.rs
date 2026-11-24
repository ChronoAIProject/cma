//! Operator queueing uses the owner's existing retained-home update state machine.
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityTarget, Capability, DetailKey,
    SandboxId,
};

use super::super::{
    instances_openapi_shapes::SandboxView,
    instances_problem::HierarchyProblem,
    sandbox_runtime_update::{problem, SandboxRuntimeUpdateView, UpdateSandboxRuntimeRequest},
};
use super::AdminState;
use crate::AuthenticatedCaller;

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new().route("/api/v1/admin/sandboxes/{id}/runtime-update", post(update))
}

#[utoipa::path(post, path = "/api/v1/admin/sandboxes/{id}/runtime-update", tag = "administration",
    params(("id" = String, Path)),
    request_body = UpdateSandboxRuntimeRequest,
    responses((status = 202, body = SandboxView), (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument)))]
pub(crate) async fn update(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    body: Result<Json<UpdateSandboxRuntimeRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<SandboxView>), HierarchyProblem> {
    if !caller.authorization.allows(Capability::ControlAnySandbox) {
        return Err(HierarchyProblem::Forbidden);
    }
    let Json(body) = body.map_err(|_| HierarchyProblem::Invalid("invalid_runtime_update"))?;
    let service = state
        .runtime_updates
        .as_ref()
        .ok_or(HierarchyProblem::Unavailable)?;
    let sandbox = service
        .request_as_operator(&caller, &SandboxId(id), &body.target_version)
        .await
        .map_err(problem)?;
    let mut record = ActivityRecord::new(ActivityAction::Update, ActivityOutcome::Succeeded)
        .by(caller.subject)
        .on(ActivityTarget::Resource(format!("sandbox:{}", sandbox.id)));
    record
        .detail
        .record(DetailKey::Subject, sandbox.owner.as_str());
    record
        .detail
        .record(DetailKey::Status, sandbox.status.name());
    record
        .detail
        .record(DetailKey::Revision, sandbox.revision.to_string());
    // A reopen stalled on a superseded target is admitted onto the latest
    // runtime; audit the target that was actually admitted or queued.
    let target = sandbox
        .admitted_runtime_update_target()
        .or_else(|| {
            sandbox
                .runtime_update_request()
                .map(|r| r.target_version.as_str())
        })
        .unwrap_or(&body.target_version);
    record.detail.record(DetailKey::Runtime, target);
    record.detail.record(DetailKey::Setting, "runtime_update");
    state
        .activity
        .record(&record)
        .await
        .map_err(|_| HierarchyProblem::Unavailable)?;
    let mut view = SandboxView::from(&sandbox);
    view.runtime_update = Some(SandboxRuntimeUpdateView::new(&sandbox, service));
    Ok((StatusCode::ACCEPTED, Json(view)))
}
