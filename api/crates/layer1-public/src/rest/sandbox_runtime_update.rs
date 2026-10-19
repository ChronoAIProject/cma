//! Owner-authorized software updates retaining the Sandbox and its home.
use super::idempotency::{IdempotencyKey, RequestBody};
use super::instances::{execute_command, HierarchyState};
use super::instances_openapi_shapes::SandboxView;
use super::instances_problem::HierarchyProblem;
use super::BodyProblem;
use crate::AuthenticatedCaller;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::sandbox::{
    Sandbox, SandboxRuntimeUpdateError, SandboxRuntimeUpdateService, SandboxStatus,
};
use layer2_core::{Capability, SandboxId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSandboxRuntimeRequest {
    pub target_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxRuntimeUpdateView {
    pub current_version: Option<String>,
    pub latest_version: String,
    pub update_available: bool,
    pub state: SandboxRuntimeUpdateStatus,
    pub can_update: bool,
    pub preserves_home: bool,
    pub schedule: RuntimeUpdateScheduleView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<RuntimeUpdateFailureView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SandboxRuntimeUpdateStatus {
    UpToDate,
    Available,
    Queued,
    Updating,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RuntimeUpdateScheduleView {
    pub timezone: String,
    pub local_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RuntimeUpdateFailureView {
    pub code: String,
    pub message: String,
}

impl SandboxRuntimeUpdateView {
    pub(super) fn new(sandbox: &Sandbox, service: &SandboxRuntimeUpdateService) -> Self {
        let current_version = sandbox
            .retained_runtime_binding()
            .map(|runtime| runtime.runtime_version.clone());
        let same_home = sandbox.supports_retained_runtime_update();
        let update_available = same_home
            && current_version
                .as_deref()
                .is_some_and(|version| version != service.latest_version());
        let request = sandbox.runtime_update_request();
        let pending = request.filter(|request| request.completed_at.is_none());
        let state = match pending {
            Some(request) if request.failure.is_some() => SandboxRuntimeUpdateStatus::Failed,
            Some(request) if request.started_at.is_some() => SandboxRuntimeUpdateStatus::Updating,
            Some(_) => SandboxRuntimeUpdateStatus::Queued,
            None if update_available => SandboxRuntimeUpdateStatus::Available,
            None => SandboxRuntimeUpdateStatus::UpToDate,
        };
        let blocked_reason = if sandbox.profile.content.runtime_image.is_some() {
            Some("This Sandbox uses a custom image. Automatic CMA runtime updates do not replace custom images.".into())
        } else if !same_home {
            Some("Runtime updates require a retained managed home and its original runtime identity.".into())
        } else if pending.is_some_and(|request| request.failure.is_none()) {
            Some(
                if state == SandboxRuntimeUpdateStatus::Queued {
                    "Waiting for all active turns and queued messages to finish."
                } else {
                    "Updating runtime software while keeping the same home and conversations."
                }
                .into(),
            )
        } else if !matches!(
            sandbox.status,
            SandboxStatus::Active | SandboxStatus::Paused
        ) && pending.is_none()
        {
            Some("Wait for the current Sandbox operation to finish.".into())
        } else {
            None
        };
        Self {
            current_version, latest_version: service.latest_version().into(), update_available, state,
            can_update: service.can_update(sandbox), preserves_home: same_home,
            schedule: RuntimeUpdateScheduleView { timezone: "Asia/Singapore".into(), local_time: "02:00".into() },
            blocked_reason, target_version: request.map(|request| request.target_version.clone()),
            requested_at: request.map(|request| request.requested_at.clone()),
            started_at: request.and_then(|request| request.started_at.clone()),
            completed_at: request.and_then(|request| request.completed_at.clone()),
            failure: pending.and_then(|request| request.failure.as_ref()).map(|failure| RuntimeUpdateFailureView {
                code: failure.class.clone(),
                message: "The runtime update paused safely. Your home and conversations are retained. Retry the update to continue.".into(),
            }),
        }
    }
}

#[utoipa::path(post, path = "/api/v1/sandboxes/{id}/runtime-update", tag = "sandboxes",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = UpdateSandboxRuntimeRequest,
    responses((status = 202, body = SandboxView), (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument)))]
pub(super) async fn update(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<UpdateSandboxRuntimeRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let command_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        Capability::UseOwnSandboxes,
        key,
        format!("sandbox:runtime-update:{id}"),
        body,
        move |body| async move {
            let service = command_state
                .runtime_updates
                .as_ref()
                .ok_or(HierarchyProblem::Unavailable)?;
            let sandbox = service
                .request(&command_caller, &SandboxId(id), &body.target_version)
                .await
                .map_err(problem)?;
            let mut view = SandboxView::from(&sandbox);
            view.runtime_update = Some(SandboxRuntimeUpdateView::new(&sandbox, service));
            Ok((StatusCode::ACCEPTED, Json(view)).into_response())
        },
    )
    .await
}

fn problem(error: SandboxRuntimeUpdateError) -> HierarchyProblem {
    match error {
        SandboxRuntimeUpdateError::Forbidden => HierarchyProblem::Forbidden,
        SandboxRuntimeUpdateError::NotFound => HierarchyProblem::SandboxNotFound,
        SandboxRuntimeUpdateError::TargetChanged => {
            HierarchyProblem::Conflict("runtime_update_target_changed")
        }
        SandboxRuntimeUpdateError::NotAvailable => {
            HierarchyProblem::Conflict("runtime_update_not_available")
        }
        SandboxRuntimeUpdateError::Unavailable => HierarchyProblem::Unavailable,
    }
}
