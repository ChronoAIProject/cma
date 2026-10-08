//! Creator-authenticated routing policy operations.
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use utoipa::IntoParams;

use super::{
    management::ChannelResponse,
    management_responses::{
        channel_response, management_error, parse_channel_id, provider_response,
    },
    problems::{no_store, problem},
};
use crate::{
    auth::Creator,
    channels::{
        management::{LoadedChannel, ManagementState},
        routing_policy::{self, RoutingPutRequest},
        ChannelTarget,
    },
};
use cmaeg_core::problem::ProblemCode;

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct RemoveRoutingQuery {
    #[param(required = true, nullable = false)]
    pub expected_version: Option<u64>,
    #[serde(default)]
    pub cascade: bool,
}

#[utoipa::path(
    put,
    path = "/v1/channels/{channel_id}/routing",
    operation_id = "putChannelRouting",
    tag = "channels",
    security(("creatorBearer" = [])),
    params(
        ("channel_id" = String, Path),
        ("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator")
    ),
    request_body = RoutingPutRequest,
    responses((status = 200, body = ChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn put(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw): Path<String>,
    body: Result<Json<RoutingPutRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let id = match parse_channel_id(&raw) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    let Ok(Json(body)) = body else {
        return invalid("request body is invalid");
    };
    match routing_policy::put(&state, &creator, &ChannelTarget::Id(id), body).await {
        Ok(loaded) => response(&state, loaded),
        Err(error) => management_error(error),
    }
}

#[utoipa::path(
    delete,
    path = "/v1/channels/{channel_id}/routing",
    operation_id = "removeChannelRouting",
    tag = "channels",
    security(("creatorBearer" = [])),
    params(
        ("channel_id" = String, Path),
        ("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"),
        RemoveRoutingQuery
    ),
    responses((status = 200, body = ChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn remove(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw): Path<String>,
    query: Result<Query<RemoveRoutingQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let id = match parse_channel_id(&raw) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    let Ok(Query(query)) = query else {
        return invalid("query parameters are invalid");
    };
    let Some(version) = query.expected_version else {
        return invalid("expected_version is required");
    };
    match routing_policy::remove(
        &state,
        &creator,
        &ChannelTarget::Id(id),
        version,
        query.cascade,
    )
    .await
    {
        Ok(loaded) => response(&state, loaded),
        Err(error) => management_error(error),
    }
}

pub(super) fn response(state: &ManagementState, loaded: LoadedChannel) -> Response {
    match channel_response(
        state,
        &loaded.channel_id,
        &loaded.definition,
        loaded.record.version,
        provider_response(&loaded.definition),
        &loaded.pointer.record_id,
    ) {
        Ok(response) => no_store(Json(response).into_response()),
        Err(error) => management_error(error),
    }
}

fn invalid(detail: &str) -> Response {
    no_store(problem(
        StatusCode::BAD_REQUEST,
        ProblemCode::InvalidRequest,
        detail,
    ))
}
