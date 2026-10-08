//! Creator-authenticated consumer mutation routes and public projections.
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
        consumer_management::{self, ManagedConsumer},
        consumer_requests::{ConsumerCreateRequest, ConsumerUpdateRequest},
        management::{ManagementError, ManagementState},
        ChannelTarget,
    },
    providers::FieldRefusal,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::{
    channel::{ConformanceLevel, ProviderTarget},
    ids::{BindingId, ConsumerId, ProviderSlug},
    problem::ProblemCode,
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConsumerResponse {
    pub id: ConsumerId,
    pub name: String,
    /// Effective label: the stored label, otherwise the consumer name.
    pub label: String,
    pub slug: ProviderSlug,
    pub target: ProviderTarget,
    pub binding_id: BindingId,
    pub level: ConformanceLevel,
    pub profile_digest: String,
    pub agent_key_hash: String,
    pub pending: bool,
    pub non_human: Option<bool>,
    pub non_human_effective: bool,
    pub honoured: Vec<String>,
    pub refused: Vec<FieldRefusal>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConsumerChannelResponse {
    pub channel: ChannelResponse,
    pub consumer: ConsumerResponse,
}

#[utoipa::path(
    post, path = "/v1/channels/{channel_id}/consumers", operation_id = "addChannelConsumer", tag = "channels",
    security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator")),
    request_body = ConsumerCreateRequest,
    responses((status = 201, body = ConsumerChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn add(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw): Path<String>,
    body: Result<Json<ConsumerCreateRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let id = match parse_channel_id(&raw) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    let Ok(Json(body)) = body else {
        return invalid("request body is invalid");
    };
    match consumer_management::add(&state, &creator, &ChannelTarget::Id(id), body).await {
        Ok(result) => response(&state, result, StatusCode::CREATED),
        Err(error) => management_error(error),
    }
}

#[utoipa::path(
    put, path = "/v1/channels/{channel_id}/consumers/{consumer_id}", operation_id = "updateChannelConsumer", tag = "channels",
    security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("consumer_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator")),
    request_body = ConsumerUpdateRequest,
    responses((status = 200, body = ConsumerChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn update(
    State(state): State<ManagementState>,
    creator: Creator,
    Path((raw, consumer)): Path<(String, String)>,
    body: Result<Json<ConsumerUpdateRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let (target, consumer) = match parse(&raw, &consumer) {
        Ok(value) => value,
        Err(error) => return management_error(error),
    };
    let Ok(Json(body)) = body else {
        return invalid("request body is invalid");
    };
    match consumer_management::update(&state, &creator, &target, &consumer, body).await {
        Ok(result) => response(&state, result, StatusCode::OK),
        Err(error) => management_error(error),
    }
}

#[utoipa::path(
    delete, path = "/v1/channels/{channel_id}/consumers/{consumer_id}", operation_id = "removeChannelConsumer", tag = "channels",
    security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("consumer_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), DeleteQuery),
    responses((status = 200, body = ChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn remove(
    State(state): State<ManagementState>,
    creator: Creator,
    Path((raw, consumer)): Path<(String, String)>,
    query: Result<Query<DeleteQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let (target, consumer) = match parse(&raw, &consumer) {
        Ok(value) => value,
        Err(error) => return management_error(error),
    };
    let Ok(Query(query)) = query else {
        return invalid("query parameters are invalid");
    };
    let Some(version) = query.expected_version else {
        return invalid("expected_version is required");
    };
    match consumer_management::remove(&state, &creator, &target, &consumer, version).await {
        Ok(loaded) => super::routing::response(&state, loaded),
        Err(error) => management_error(error),
    }
}

fn response(state: &ManagementState, result: ManagedConsumer, status: StatusCode) -> Response {
    let loaded = result.channel;
    let Some(view) = loaded.definition.consumer(Some(&result.consumer_id)) else {
        return management_error(ManagementError::NotFound);
    };
    let consumer = ConsumerResponse {
        id: view.id.clone(),
        name: view.name.into(),
        label: view.label.into(),
        slug: view.provider.slug.clone(),
        target: view.provider.target.clone(),
        binding_id: view.provider.binding_id.clone(),
        level: view.provider.level,
        profile_digest: view.profile_digest.into(),
        agent_key_hash: view.agent_key_hash.into(),
        pending: view.pending,
        non_human: view.non_human,
        non_human_effective: view.non_human_effective(&state.routing.non_human_providers),
        honoured: result
            .binding
            .as_ref()
            .map_or_else(Vec::new, |binding| binding.honoured.clone()),
        refused: result
            .binding
            .map_or_else(Vec::new, |binding| binding.refused),
    };
    match channel_response(
        state,
        &loaded.channel_id,
        &loaded.definition,
        loaded.record.version,
        provider_response(&loaded.definition),
        &loaded.pointer.record_id,
    ) {
        Ok(channel) => {
            no_store((status, Json(ConsumerChannelResponse { channel, consumer })).into_response())
        }
        Err(error) => management_error(error),
    }
}

fn parse(raw: &str, consumer: &str) -> Result<(ChannelTarget, ConsumerId), ManagementError> {
    let channel = parse_channel_id(raw)?;
    let consumer = ConsumerId::parse(consumer).map_err(|mut error| {
        error.pointer = "/consumer_id".into();
        ManagementError::Invalid(error)
    })?;
    Ok((ChannelTarget::Id(channel), consumer))
}

fn invalid(detail: &str) -> Response {
    no_store(problem(
        StatusCode::BAD_REQUEST,
        ProblemCode::InvalidRequest,
        detail,
    ))
}
