//! Read-only Trigger companions for AG-UI clients.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::auth::AuthenticatedCaller;
use crate::rest::triggers::{
    load_diff_review, load_launch, load_launch_progress, load_public_preview, DiffReviewView,
    LaunchProgressView, LaunchView, TriggerPreviewView, TriggerProblem,
};
use crate::rest::{RequestFailure, TriggersState};

const DEFAULT_PROGRESS_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgressParams {
    #[serde(default)]
    after: u64,
    #[serde(default = "default_progress_limit")]
    limit: usize,
}

const fn default_progress_limit() -> usize {
    DEFAULT_PROGRESS_LIMIT
}

pub(crate) fn router(state: TriggersState) -> Router {
    Router::new()
        .route("/api/v1/agui/triggers/{public_id}", get(preview))
        .route("/api/v1/agui/launches/{launch_id}", get(launch))
        .route("/api/v1/agui/launches/{launch_id}/events", get(progress))
        .route("/api/v1/agui/launches/{launch_id}/diff", get(diff))
        .with_state(state)
}

async fn preview(
    State(state): State<TriggersState>,
    Path(public_id): Path<String>,
) -> Result<Json<TriggerPreviewView>, TriggerProblem> {
    Ok(Json(load_public_preview(&state, public_id).await?))
}

async fn launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
) -> Result<Json<LaunchView>, TriggerProblem> {
    Ok(Json(load_launch(&state, &caller.subject, launch_id).await?))
}

async fn progress(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    params: Result<Query<ProgressParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<LaunchProgressView>, RequestFailure> {
    let Query(params) = params?;
    Ok(Json(
        load_launch_progress(
            &state,
            &caller.subject,
            launch_id,
            params.after,
            params.limit,
        )
        .await?,
    ))
}

async fn diff(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
) -> Result<Json<DiffReviewView>, TriggerProblem> {
    Ok(Json(
        load_diff_review(&state, &caller.subject, launch_id).await?,
    ))
}
