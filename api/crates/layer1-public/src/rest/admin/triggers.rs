//! Deployment-wide Trigger orchestration controls.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};

use crate::AuthenticatedCaller;

use super::settings::{self, SettingsView};
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
    let stored = state.settings.pause_triggers(&caller).await?;
    Ok(Json(
        settings::view(stored, &state.settings, &state.users, &caller).await,
    ))
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
    let stored = state.settings.resume_triggers(&caller).await?;
    Ok(Json(
        settings::view(stored, &state.settings, &state.users, &caller).await,
    ))
}
