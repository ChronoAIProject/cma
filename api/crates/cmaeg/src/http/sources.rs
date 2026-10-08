//! Creator-authenticated source mutations, projected without source credentials.
use super::{
    management::{ChannelResponse, DeleteQuery},
    management_responses::{
        channel_response, management_error, parse_channel_id, provider_response,
    },
    problems::{no_store, problem},
};
use crate::{
    auth::Creator,
    channels::{
        management::ManagementState,
        sources::{self, SourceAttachRequest},
        ChannelTarget,
    },
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::{channel::SourceId, problem::ProblemCode};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SourceAttachResponse {
    #[serde(flatten)]
    pub channel: ChannelResponse,
    pub source_id: SourceId,
}

#[utoipa::path(
    post, path = "/v1/channels/{channel_id}/sources", operation_id = "attachChannelSource", tag = "channels",
    security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator")),
    request_body = SourceAttachRequest,
    responses((status = 201, body = SourceAttachResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn attach(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw): Path<String>,
    body: Result<Json<SourceAttachRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let id = match parse_channel_id(&raw) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    let Ok(Json(body)) = body else {
        return invalid("request body is invalid");
    };
    match sources::attach(&state, &creator, &ChannelTarget::Id(id), body).await {
        Ok((loaded, source_id)) => match channel_response(
            &state,
            &loaded.channel_id,
            &loaded.definition,
            loaded.record.version,
            provider_response(&loaded.definition),
            &loaded.pointer.record_id,
        ) {
            Ok(channel) => no_store(
                (
                    StatusCode::CREATED,
                    Json(SourceAttachResponse { channel, source_id }),
                )
                    .into_response(),
            ),
            Err(error) => management_error(error),
        },
        Err(error) => management_error(error),
    }
}

#[utoipa::path(
    delete, path = "/v1/channels/{channel_id}/sources/{source_id}", operation_id = "detachChannelSource", tag = "channels",
    security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("source_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), DeleteQuery),
    responses((status = 200, body = ChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn detach(
    State(state): State<ManagementState>,
    creator: Creator,
    Path((raw, source)): Path<(String, String)>,
    query: Result<Query<DeleteQuery>, axum::extract::rejection::QueryRejection>,
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
    match sources::detach_named(&state, &creator, &ChannelTarget::Id(id), &source, version).await {
        Ok(loaded) => super::routing::response(&state, loaded),
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
