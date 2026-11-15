//! Operator preview and explicit idle-pause controls.
use std::sync::Arc;

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use layer2_core::sandbox::store::{PageRequest, SandboxMaintenanceOutcome};
use layer2_core::{now_rfc3339, Capability};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{AdminState, CollectionProblem};
use crate::AuthenticatedCaller;

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AutoPausePreviewView {
    /// Active and paused Sandboxes visible to the maintenance scanner. The
    /// reconciler still revalidates activity and Always on before mutation.
    pub candidates: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PauseIdleNowRequest {
    pub confirmed: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PauseIdleNowView {
    pub scheduled: u64,
    pub already_satisfied: u64,
    pub skipped: u64,
}

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new()
        .route("/api/v1/admin/auto-pause/preview", get(preview))
        .route(
            "/api/v1/admin/auto-pause/pause-idle-now",
            post(pause_idle_now),
        )
}

#[utoipa::path(get, path = "/api/v1/admin/auto-pause/preview", tag = "administration", responses((status = 200, body = AutoPausePreviewView)))]
pub(crate) async fn preview(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<AutoPausePreviewView>, CollectionProblem> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(CollectionProblem::from)?;
    let store = state
        .maintenance
        .as_ref()
        .ok_or(CollectionProblem::Unavailable)?;
    let threshold = state
        .settings
        .current()
        .await
        .map_err(CollectionProblem::from)?
        .settings
        .inactivity
        .pause_after_seconds;
    let mut page = PageRequest::default();
    let mut candidates = 0u64;
    loop {
        let result = store
            .sandbox_maintenance_candidates(&page)
            .await
            .map_err(|_| CollectionProblem::Unavailable)?;
        for sandbox_id in result.items {
            let outcome = store
                .preview_inactivity_pause(
                    layer2_core::sandbox::store::SandboxInactivityPauseRequest {
                        sandbox_id,
                        requested_at: now_rfc3339(),
                        pause_after_seconds: threshold,
                        auto_pause_enabled: true,
                        cause: Default::default(),
                    },
                )
                .await
                .map_err(|_| CollectionProblem::Unavailable)?;
            if outcome == SandboxMaintenanceOutcome::Scheduled {
                candidates = candidates.saturating_add(1);
            }
        }
        let Some(next) = result.next else { break };
        page.cursor = Some(next);
    }
    Ok(Json(AutoPausePreviewView { candidates }))
}

#[utoipa::path(post, path = "/api/v1/admin/auto-pause/pause-idle-now", tag = "administration", request_body = PauseIdleNowRequest, responses((status = 200, body = PauseIdleNowView), (status = 400), (status = 503)))]
pub(crate) async fn pause_idle_now(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Json(body): Json<PauseIdleNowRequest>,
) -> Result<Json<PauseIdleNowView>, CollectionProblem> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(CollectionProblem::from)?;
    if !body.confirmed {
        return Err(CollectionProblem::InvalidRequest);
    }
    let store = state
        .maintenance
        .as_ref()
        .ok_or(CollectionProblem::Unavailable)?;
    let threshold = state
        .settings
        .current()
        .await
        .map_err(CollectionProblem::from)?
        .settings
        .inactivity
        .pause_after_seconds;
    let requested_at = now_rfc3339();
    let mut page = PageRequest::default();
    let mut view = PauseIdleNowView {
        scheduled: 0,
        already_satisfied: 0,
        skipped: 0,
    };
    loop {
        let result = store
            .sandbox_maintenance_candidates(&page)
            .await
            .map_err(|_| CollectionProblem::Unavailable)?;
        for sandbox_id in result.items {
            let decision_id = sandbox_id.clone();
            let outcome = store
                .schedule_inactivity_pause(
                    layer2_core::sandbox::store::SandboxInactivityPauseRequest {
                        sandbox_id,
                        requested_at: requested_at.clone(),
                        pause_after_seconds: threshold,
                        auto_pause_enabled: true,
                        cause: Default::default(),
                    },
                )
                .await
                .map_err(|_| CollectionProblem::Unavailable)?;
            tracing::info!(
                sandbox_id = %decision_id,
                outcome = ?outcome,
                threshold_seconds = threshold,
                "operator requested idle sandbox pause decision"
            );
            match outcome {
                SandboxMaintenanceOutcome::Scheduled => view.scheduled += 1,
                SandboxMaintenanceOutcome::AlreadyScheduled => view.already_satisfied += 1,
                SandboxMaintenanceOutcome::NotEligible => view.skipped += 1,
            }
        }
        let Some(next) = result.next else { break };
        page.cursor = Some(next);
    }
    Ok(Json(view))
}
