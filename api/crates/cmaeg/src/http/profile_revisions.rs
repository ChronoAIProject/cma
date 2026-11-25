//! Owner-only channel profile revision operations.
use super::{
    management_responses::{management_error, parse_channel_id},
    problems::no_store,
};
use crate::{
    auth::Creator,
    channels::{
        management::ManagementState,
        profile_revision::{self, RevisionRequest},
    },
};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::{post, put},
    Json, Router,
};
use serde::Deserialize;
use utoipa::ToSchema;

pub(super) fn routes() -> Router<ManagementState> {
    Router::new()
        .route(
            "/v1/channels/{channel_id}/profile-revisions/baseline",
            post(baseline),
        )
        .route("/v1/channels/{channel_id}/profile-revisions", put(publish))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BaselineRequest {
    pub expected_version: u64,
}

#[utoipa::path(post, path = "/v1/channels/{channel_id}/profile-revisions/baseline", operation_id = "baselineChannelProfileRevisions", tag = "channels", security(("creatorBearer" = [])), params(("channel_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header)), request_body = BaselineRequest, responses((status = 200, body = profile_revision::RevisionReceipt), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json")))]
pub async fn baseline(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw): Path<String>,
    Json(request): Json<BaselineRequest>,
) -> Response {
    let id = match parse_channel_id(&raw) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    match profile_revision::baseline(&state, &creator, &id, request.expected_version).await {
        Ok(receipt) => no_store(Json(receipt).into_response()),
        Err(error) => management_error(error),
    }
}

#[utoipa::path(put, path = "/v1/channels/{channel_id}/profile-revisions", operation_id = "publishChannelProfileRevision", tag = "channels", security(("creatorBearer" = [])), params(("channel_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header)), request_body = RevisionRequest, responses((status = 200, body = profile_revision::RevisionReceipt), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json")))]
pub async fn publish(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw): Path<String>,
    Json(request): Json<RevisionRequest>,
) -> Response {
    let id = match parse_channel_id(&raw) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    match profile_revision::publish(&state, &creator, &id, request).await {
        Ok(receipt) => no_store(Json(receipt).into_response()),
        Err(error) => management_error(error),
    }
}
