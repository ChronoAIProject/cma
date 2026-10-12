//! Owner-scoped recovery of a reserved channel after a lost create response.
use super::{
    management::ChannelResponse,
    management_responses::{channel_response, management_error, provider_response},
    problems::no_store,
};
use crate::{
    auth::Creator,
    channels::{
        creation,
        management::{ManagementError, ManagementState},
    },
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::ids::RecordId;

#[utoipa::path(get, path="/v1/channel-records/{record_id}", operation_id="recoverChannel", tag="channels",
    security(("creatorBearer"=[])), params(("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), ("record_id"=String, Path)),
    responses((status=200, body=ChannelResponse), (status="default", body=cmaeg_core::problem::Problem, content_type="application/problem+json")))]
pub async fn recover_channel(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(id): Path<String>,
) -> Response {
    let id = match RecordId::new(id) {
        Ok(id) => id,
        Err(error) => return management_error(ManagementError::Invalid(error)),
    };
    let current = match creation::load_record(&state, &creator, &id).await {
        Ok(value) => value,
        Err(error) => return management_error(error),
    };
    match channel_response(
        &state,
        &current.channel_id,
        &current.definition,
        current.record.version,
        provider_response(&current.definition),
        &id,
    ) {
        Ok(value) => no_store((StatusCode::OK, Json(value)).into_response()),
        Err(error) => management_error(error),
    }
}
