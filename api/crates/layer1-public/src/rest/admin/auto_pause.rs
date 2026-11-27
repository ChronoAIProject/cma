//! Operator preview and explicit idle-pause controls.
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use layer2_core::sandbox::store::PageRequest;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{AdminState, CollectionProblem};
use crate::AuthenticatedCaller;

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AutoPausePreviewView {
    /// Eligible Sandboxes in this bounded page; follow next for further pages.
    pub candidates: u64,
    /// Fixed refusal codes, present when a Sandbox was refused on this page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refusals: Vec<AutoPauseSandboxRefusalView>,
    pub failed: u64,
    pub next: Option<String>,
    pub enabled: bool,
    pub policy_revision: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AutoPauseSandboxRefusalView {
    pub sandbox_id: String,
    #[schema(value_type = String)]
    pub reason: layer2_core::sandbox::InactivityPauseRefusal,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PauseIdleNowRequest {
    pub confirmed: bool,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PauseIdleNowView {
    pub scheduled: u64,
    pub already_satisfied: u64,
    pub skipped: u64,
    pub failed: u64,
    pub next: Option<String>,
    pub enabled: bool,
    pub policy_revision: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AutoPausePage {
    cursor: Option<String>,
    limit: Option<u16>,
}

fn page(limit: Option<u16>, cursor: Option<String>) -> Result<PageRequest, CollectionProblem> {
    PageRequest::new(
        limit.unwrap_or(layer2_core::sandbox::AUTO_PAUSE_BATCH_LIMIT),
        cursor,
    )
    .map_err(|_| CollectionProblem::InvalidRequest)
}

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new()
        .route("/api/v1/admin/auto-pause/preview", get(preview))
        .route(
            "/api/v1/admin/auto-pause/pause-idle-now",
            post(pause_idle_now),
        )
}

#[utoipa::path(get, path = "/api/v1/admin/auto-pause/preview", tag = "administration", params(
    ("cursor" = Option<String>, Query, description = "Exclusive cursor; empty retries the first candidate"),
    ("limit" = Option<u16>, Query, description = "Batch size, 1 through 100; default 25")
), responses((status = 200, body = AutoPausePreviewView), (status = 400), (status = 403), (status = 503)))]
pub(crate) async fn preview(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Query(params): Query<AutoPausePage>,
) -> Result<Json<AutoPausePreviewView>, CollectionProblem> {
    let batch = state
        .auto_pause
        .as_ref()
        .ok_or(CollectionProblem::Unavailable)?
        .batch(&caller, page(params.limit, params.cursor)?, false)
        .await
        .map_err(CollectionProblem::from)?;
    Ok(Json(AutoPausePreviewView {
        candidates: batch.candidates,
        refusals: batch
            .refusals
            .into_iter()
            .map(|refusal| AutoPauseSandboxRefusalView {
                sandbox_id: refusal.sandbox_id.to_string(),
                reason: refusal.reason,
            })
            .collect(),
        failed: batch.failed,
        next: batch.next,
        enabled: batch.enabled,
        policy_revision: batch.policy_revision,
    }))
}

#[utoipa::path(post, path = "/api/v1/admin/auto-pause/pause-idle-now", tag = "administration", request_body = PauseIdleNowRequest, responses((status = 200, body = PauseIdleNowView), (status = 400), (status = 503)))]
pub(crate) async fn pause_idle_now(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Json(body): Json<PauseIdleNowRequest>,
) -> Result<Json<PauseIdleNowView>, CollectionProblem> {
    if !body.confirmed {
        return Err(CollectionProblem::InvalidRequest);
    }
    let batch = state
        .auto_pause
        .as_ref()
        .ok_or(CollectionProblem::Unavailable)?
        .batch(&caller, page(body.limit, body.cursor)?, true)
        .await
        .map_err(CollectionProblem::from)?;
    Ok(Json(PauseIdleNowView {
        scheduled: batch.scheduled,
        already_satisfied: batch.already_satisfied,
        skipped: batch.skipped,
        failed: batch.failed,
        next: batch.next,
        enabled: batch.enabled,
        policy_revision: batch.policy_revision,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::sandbox::InactivityPauseRefusal;

    #[test]
    fn preview_includes_the_sandbox_and_code_when_refused() {
        let view = AutoPausePreviewView {
            candidates: 0,
            refusals: vec![AutoPauseSandboxRefusalView {
                sandbox_id: "sbx_fixture".into(),
                reason: InactivityPauseRefusal::RecentAgentActivity,
            }],
            failed: 0,
            next: None,
            enabled: true,
            policy_revision: 1,
        };
        assert_eq!(
            serde_json::to_value(view).unwrap()["refusals"],
            serde_json::json!([
                {"sandbox_id": "sbx_fixture", "reason": "recent_agent_activity"}
            ])
        );
    }
}
