//! Public per-node diagnostics and policy updates used by all clients.
use super::{
    management::{ChannelResponse, TestChannelResponse},
    management_responses::{
        channel_response, management_error, parse_channel_id, provider_response,
    },
    problems::{no_store, problem},
};
use crate::{
    auth::Creator,
    channels::{
        management::{self, ManagementState},
        routing_update::{self, RoutingUpdateRequest},
        source_check,
    },
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::{ids::ConsumerId, problem::ProblemCode};

#[utoipa::path(post, path = "/v1/channels/{channel_id}/sources/{source_index}/test", operation_id = "testChannelSource", tag = "channels", security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("source_index" = usize, Path)),
    responses((status = 200, body = source_check::SourceCheckResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json")))]
pub async fn test_source(
    State(state): State<ManagementState>,
    creator: Creator,
    Path((raw_id, index)): Path<(String, usize)>,
) -> Response {
    let id = match parse_channel_id(&raw_id) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    match source_check::check(&state, &creator, &id, index).await {
        Ok(body) => no_store(Json(body).into_response()),
        Err(error) => management_error(error),
    }
}

#[utoipa::path(post, path = "/v1/channels/{channel_id}/consumers/{consumer_id}/test", operation_id = "testChannelConsumer", tag = "channels", security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("consumer_id" = String, Path)),
    responses((status = 200, body = TestChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json")))]
pub async fn test_consumer(
    State(state): State<ManagementState>,
    creator: Creator,
    Path((raw_id, raw_consumer)): Path<(String, String)>,
) -> Response {
    let id = match parse_channel_id(&raw_id) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    let consumer = match ConsumerId::parse(&raw_consumer) {
        Ok(id) => id,
        Err(error) => return management_error(management::ManagementError::Invalid(error)),
    };
    match management::test_consumer(&state, &creator, &id, Some(&consumer)).await {
        Ok(body) => no_store(
            Json(TestChannelResponse {
                accepted_after_ms: body.accepted_after_ms,
                first_text: body.first_text,
                level: body.level,
            })
            .into_response(),
        ),
        Err(error) => management_error(error),
    }
}

#[utoipa::path(put, path = "/v1/channels/{channel_id}/routing", operation_id = "updateChannelRouting", tag = "channels", security(("creatorBearer" = [])),
    params(("channel_id" = String, Path)), request_body = RoutingUpdateRequest,
    responses((status = 200, body = ChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json")))]
pub async fn update_routing(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw_id): Path<String>,
    body: Result<Json<RoutingUpdateRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let id = match parse_channel_id(&raw_id) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    let body = match body {
        Ok(Json(body)) => body,
        Err(_) => {
            return no_store(problem(
                StatusCode::BAD_REQUEST,
                ProblemCode::InvalidRequest,
                "request does not match the routing policy schema",
            ))
        }
    };
    let loaded = match routing_update::update(&state, &creator, &id, body).await {
        Ok(value) => value,
        Err(error) => return management_error(error),
    };
    channel_response(
        &state,
        &id,
        &loaded.definition,
        loaded.record.version,
        provider_response(&loaded.definition),
        &loaded.definition.record_id,
    )
    .map(|body| no_store(Json(body).into_response()))
    .unwrap_or_else(management_error)
}
