//! Deployment-wide Trigger orchestration controls.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};

use crate::AuthenticatedCaller;

use super::settings::SettingsView;
use super::{AdminState, CollectionProblem};

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new()
        .route("/api/v1/admin/triggers/pause", post(pause))
        .route("/api/v1/admin/triggers/resume", post(resume))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/triggers/pause",
    tag = "administration",
    responses((status = 200, body = SettingsView), (status = 403, body = crate::rest::ProblemDocument))
)]
pub(crate) async fn pause(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<SettingsView>, CollectionProblem> {
    state
        .settings
        .pause_triggers(&caller)
        .await
        .map(|stored| SettingsView::new(stored, &state.settings))
        .map(Json)
        .map_err(CollectionProblem::from)
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/triggers/resume",
    tag = "administration",
    responses((status = 200, body = SettingsView), (status = 403, body = crate::rest::ProblemDocument))
)]
pub(crate) async fn resume(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<SettingsView>, CollectionProblem> {
    state
        .settings
        .resume_triggers(&caller)
        .await
        .map(|stored| SettingsView::new(stored, &state.settings))
        .map(Json)
        .map_err(CollectionProblem::from)
}
