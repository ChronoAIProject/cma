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
    Sandbox, SandboxRuntimeImageOrigin, SandboxRuntimeUpdateError, SandboxRuntimeUpdateService,
    SandboxStatus,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version_confirmed: Option<bool>,
    pub image_origin: RuntimeUpdateImageOrigin,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeUpdateImageOrigin {
    ManagedAgent,
    ManagedDesktop,
    ManagedMachine,
    UserSelected,
    Unknown,
}

impl SandboxRuntimeUpdateView {
    pub(super) fn new(sandbox: &Sandbox, service: &SandboxRuntimeUpdateService) -> Self {
        // One release for the whole view, so its fields always agree.
        let latest_version = service.latest_version_for(sandbox).unwrap_or_default();
        let current_version = sandbox
            .retained_runtime_binding()
            .map(|runtime| runtime.runtime_version.clone());
        let same_home = sandbox.supports_retained_runtime_update();
        let can_update = service.can_update_to(sandbox, &latest_version);
        let qualified = service.target_is_qualified(sandbox, &latest_version);
        let current_confirmed = sandbox.managed_desktop_runtime_confirmed();
        let image_origin = match sandbox.lifecycle.runtime_image_origin {
            Some(SandboxRuntimeImageOrigin::ManagedDesktop) => {
                RuntimeUpdateImageOrigin::ManagedDesktop
            }
            Some(SandboxRuntimeImageOrigin::ManagedMachine) => {
                RuntimeUpdateImageOrigin::ManagedMachine
            }
            Some(SandboxRuntimeImageOrigin::UserSelected) => RuntimeUpdateImageOrigin::UserSelected,
            None if sandbox.has_managed_runtime_image() => RuntimeUpdateImageOrigin::ManagedAgent,
            None => RuntimeUpdateImageOrigin::Unknown,
        };
        let update_available = same_home
            && qualified
            && current_confirmed
            && current_version
                .as_deref()
                .is_some_and(|version| version != latest_version);
        let request = sandbox.runtime_update_request();
        let pending = request.filter(|request| request.completed_at.is_none());
        let state = match pending {
            Some(request) if request.failure.is_some() => SandboxRuntimeUpdateStatus::Failed,
            Some(request) if request.started_at.is_some() => SandboxRuntimeUpdateStatus::Updating,
            Some(_) => SandboxRuntimeUpdateStatus::Queued,
            None if update_available => SandboxRuntimeUpdateStatus::Available,
            None => SandboxRuntimeUpdateStatus::UpToDate,
        };
        let blocked_reason = if image_origin == RuntimeUpdateImageOrigin::UserSelected {
            Some("This Sandbox uses a custom image. Automatic CMA runtime updates do not replace custom images.".into())
        } else if image_origin == RuntimeUpdateImageOrigin::Unknown {
            Some("The original source of this Sandbox image is unknown. Managed runtime updates require a confirmed origin. If you used the platform's default image, contact the deployment operator to check this Sandbox's image origin and update eligibility.".into())
        } else if !same_home {
            Some("Runtime updates require a retained managed home and its original runtime identity.".into())
        } else if sandbox.requires_retained_runtime_transition() {
            Some("Runtime updates for this retained startup policy require a deployment operator grant.".into())
        } else if pending.is_some_and(|request| request.failure.is_none()) {
            Some(
                if state == SandboxRuntimeUpdateStatus::Queued {
                    "Waiting for all active turns and queued messages to finish."
                } else {
                    "Updating runtime software while keeping the same home and conversations."
                }
                .into(),
            )
        } else if !current_confirmed && pending.is_none() {
            Some("The current Desktop runtime could not be confirmed.".into())
        } else if !qualified && pending.is_none() {
            Some(
                if image_origin == RuntimeUpdateImageOrigin::ManagedMachine {
                    "A verified Machine runtime release is unavailable."
                } else {
                    "The latest Agent runtime has not been verified for Desktop."
                }
                .into(),
            )
        } else if pending.is_none() && !can_update {
            runtime_update_lifecycle_reason(sandbox.status).map(str::to_owned)
        } else {
            None
        };
        let failure = pending.and_then(|request| {
            let failure = request.failure.as_ref()?;
            let guidance = if can_update {
                if service.retry_uses_latest(sandbox, &request.target_version, &latest_version) {
                    "Retry the runtime update. Recovery can use the latest runtime on your retained home."
                } else {
                    "Retry the runtime update to continue with the requested runtime on your retained home."
                }
            } else if !same_home {
                "Retry requires a confirmed retained managed home. Contact the deployment operator for recovery help."
            } else if sandbox.work.lease.is_some() {
                "Retry is currently unavailable. Check the Sandbox status again after the current work finishes."
            } else {
                "Retry is unavailable. Contact the deployment operator for help recovering your retained home."
            };
            Some(RuntimeUpdateFailureView {
                code: failure.class.clone(),
                message: format!("The runtime update stopped at its current step. {guidance}"),
            })
        });
        Self {
            can_update,
            current_version,
            current_version_confirmed: sandbox
                .has_managed_desktop_image()
                .then_some(current_confirmed),
            image_origin,
            latest_version,
            update_available,
            state,
            preserves_home: same_home,
            schedule: RuntimeUpdateScheduleView {
                timezone: "Asia/Singapore".into(),
                local_time: "02:00".into(),
            },
            blocked_reason,
            target_version: request.map(|request| request.target_version.clone()),
            requested_at: request.map(|request| request.requested_at.clone()),
            started_at: request.and_then(|request| request.started_at.clone()),
            completed_at: request.and_then(|request| request.completed_at.clone()),
            failure,
        }
    }
}

pub(super) fn runtime_update_lifecycle_reason(status: SandboxStatus) -> Option<&'static str> {
    Some(match status {
        SandboxStatus::Provisioning => "Runtime updates are unavailable while the Sandbox is provisioning.",
        SandboxStatus::Installing => "Runtime updates are unavailable while the Sandbox is installing software.",
        SandboxStatus::Pausing => "Runtime updates are unavailable while the Sandbox is pausing.",
        SandboxStatus::Reviving => "Runtime updates are unavailable while the Sandbox is resuming.",
        SandboxStatus::CredentialRotating => "Runtime updates are unavailable during Sandbox credential rotation.",
        SandboxStatus::Rotating => "Runtime updates are unavailable during Sandbox runtime rotation.",
        SandboxStatus::Failed => "Runtime updates are blocked by the Sandbox failure. Open Technical details for the reported failure.",
        SandboxStatus::Retiring => "The Sandbox is retiring. Runtime updates are unavailable.",
        SandboxStatus::Retired => "The Sandbox is retired. Runtime updates are unavailable.",
        SandboxStatus::Active | SandboxStatus::Paused => return None,
    })
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

pub(super) fn problem(error: SandboxRuntimeUpdateError) -> HierarchyProblem {
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
