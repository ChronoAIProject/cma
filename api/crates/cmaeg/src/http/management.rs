//! Versioned channel management routes.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::{channel::ConformanceLevel, ids::ChannelId, problem::ProblemCode};
use cmaeg_engine::management::{ChannelTest, ManagementFailure};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};
use utoipa::{IntoParams, ToSchema};

use crate::{
    auth::Creator,
    channels::{
        definition::{ChannelCreateRequest, ChannelUpdateRequest},
        management::{self, ManagementError, ManagementState},
    },
};

use super::management_responses::*;
use super::problems::{no_store, problem};

#[path = "management_router.rs"]
mod router;
pub use router::management_router;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ChannelResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_discord_activation: Option<crate::discord::activation::NativeDiscordActivation>,
    /// Explicit opt-in support; absent/empty means unsupported.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub thread_contract_versions: Vec<u16>,
    pub channel_id: String,
    pub record_id: String,
    pub version: u64,
    pub provider: ProviderResponse,
    pub endpoints: EndpointResponse,
    pub definition: super::schema::PublicChannelDefinition,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProviderResponse {
    pub slug: String,
    pub binding_id: String,
    pub level: ConformanceLevel,
    pub honoured: Vec<String>,
    pub refused: Vec<crate::providers::FieldRefusal>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct EndpointResponse {
    /// Configure only on a separately provisioned native app. Never replace a NyxID callback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_lark_webhook_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whatsapp_webhook_url: Option<String>,
    pub nyxid_callback_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_webhook_url: Option<String>,
    pub custom_event_url: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RekeyResponse {
    pub kid: u8,
}

pub use cmaeg_engine::management::TestChannelResponse;

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct DeleteQuery {
    #[param(required = true, nullable = false)]
    pub expected_version: Option<u64>,
    /// Retain the provider cleanup intent for an incomplete channel.
    #[serde(default)]
    pub retire_unready: bool,
}

#[utoipa::path(
    post,
    path = "/v1/channels",
    operation_id = "createChannel",
    tag = "channels",
    security(("creatorBearer" = [])),
    params(("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), ("Idempotency-Key" = Option<String>, Header, description = "1..=256 ASCII bytes; identical creates replay locally for ten minutes. For a supplied record_id, replay requires the original channel to remain ready and unretired.")),
    request_body = ChannelCreateRequest,
    responses((status = 201, body = ChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn create_channel(
    State(state): State<ManagementState>,
    creator: Creator,
    headers: HeaderMap,
    body: Result<Json<ChannelCreateRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(error) => return super::management_schema::rejection(&error),
    };
    let key = match idempotency_key(&headers) {
        Ok(key) => key,
        Err(error) => return management_error(error),
    };
    let fingerprint = body.fingerprint();
    // Keep keyed creates serialized until their response is cached, so concurrent
    // requests on one replica cannot allocate two records for the same key.
    let mut cache_guard = if key.is_some() {
        Some(state.idempotency.lock().await)
    } else {
        None
    };
    if let (Some(key), Some(cache)) = (&key, cache_guard.as_mut()) {
        cache.retain(|_, entry| entry.expires_at > std::time::Instant::now());
        if let Some(entry) = cache.get(&(creator.subject.clone(), key.clone())).cloned() {
            if entry.fingerprint != fingerprint {
                return problem(
                    StatusCode::CONFLICT,
                    ProblemCode::Conflict,
                    "Idempotency-Key was used for another request",
                );
            }
            drop(cache_guard);
            if let Some(record_id) = &body.record_id {
                // A different replica can retire this identity while this
                // replica still has its original successful response.
                let retained =
                    match crate::channels::creation::load_record(&state, &creator, record_id).await
                    {
                        Ok(retained) => Some(retained),
                        Err(ManagementError::NotFound) => None,
                        Err(error) => return management_error(error),
                    };
                if !retained.is_some_and(|current| {
                    entry.body["channel_id"].as_str() == Some(current.channel_id.as_str())
                        && current.create_replayable()
                }) {
                    return problem(
                        StatusCode::CONFLICT,
                        ProblemCode::Conflict,
                        "Idempotency-Key no longer has a ready channel for this record_id",
                    );
                }
            }
            return no_store((StatusCode::CREATED, Json(entry.body)).into_response());
        }
        if cache.len() >= 10_000 {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                ProblemCode::ProviderUnavailable,
                "idempotency cache is full",
            );
        }
    }
    let created = match management::create(&state, &creator, body).await {
        Ok(created) => created,
        Err(error) => return management_error(error),
    };
    let response = channel_response(
        &state,
        &created.channel_id,
        &created.definition,
        created.version,
        ProviderResponse {
            slug: created.definition.provider.slug.to_string(),
            binding_id: created.binding.binding_id.to_string(),
            level: created.binding.level,
            honoured: created.binding.honoured.clone(),
            refused: created.binding.refused.clone(),
        },
        &created.record_id,
    );
    let response = match response {
        Ok(response) => response,
        Err(error) => return management_error(error),
    };
    let body = match serde_json::to_value(&response) {
        Ok(body) => body,
        Err(error) => return management_error(ManagementError::Internal(error.to_string())),
    };
    if let (Some(key), Some(cache)) = (key, cache_guard.as_mut()) {
        cache.insert(
            (creator.subject.clone(), key),
            crate::channels::state::CachedResponse {
                expires_at: std::time::Instant::now() + std::time::Duration::from_secs(600),
                body: body.clone(),
                fingerprint,
            },
        );
    }
    no_store((StatusCode::CREATED, Json(body)).into_response())
}

#[utoipa::path(
    get,
    path = "/v1/channels/{channel_id}",
    operation_id = "getChannel",
    tag = "channels",
    security(("creatorBearer" = [])),
    params(("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), ("channel_id" = String, Path)),
    responses((status = 200, body = ChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn get_channel(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw_id): Path<String>,
) -> Response {
    let channel_id = match parse_channel_id(&raw_id) {
        Ok(channel_id) => channel_id,
        Err(error) => return management_error(error),
    };
    let loaded = match management::inspect(&state, &creator, &channel_id).await {
        Ok((loaded, _)) => loaded,
        Err(error) => return management_error(error),
    };
    let response = channel_response(
        &state,
        &channel_id,
        &loaded.definition,
        loaded.record.version,
        provider_response(&loaded.definition),
        &loaded.definition.record_id,
    );
    response
        .map(|body| no_store(Json(body).into_response()))
        .unwrap_or_else(management_error)
}

#[utoipa::path(
    put,
    path = "/v1/channels/{channel_id}",
    operation_id = "updateChannel",
    tag = "channels",
    security(("creatorBearer" = [])),
    params(("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), ("channel_id" = String, Path)),
    request_body = ChannelUpdateRequest,
    responses((status = 200, body = ChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn update_channel(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw_id): Path<String>,
    body: Result<Json<ChannelUpdateRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let channel_id = match parse_channel_id(&raw_id) {
        Ok(channel_id) => channel_id,
        Err(error) => return management_error(error),
    };
    let body = match body {
        Ok(Json(body)) => body,
        Err(error) => return super::management_schema::rejection(&error),
    };
    let loaded = match management::update(&state, &creator, &channel_id, body).await {
        Ok(loaded) => loaded,
        Err(error) => return management_error(error),
    };
    let response = channel_response(
        &state,
        &channel_id,
        &loaded.definition,
        loaded.record.version,
        provider_response(&loaded.definition),
        &loaded.definition.record_id,
    );
    response
        .map(|body| no_store(Json(body).into_response()))
        .unwrap_or_else(management_error)
}

#[utoipa::path(
    delete,
    path = "/v1/channels/{channel_id}",
    operation_id = "deleteChannel",
    tag = "channels",
    security(("creatorBearer" = [])),
    params(("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), ("channel_id" = String, Path), DeleteQuery),
    responses((status = 204), (status = 202, description = "Incomplete channel retired, provider cleanup pending"), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn delete_channel(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw_id): Path<String>,
    query: Result<Query<DeleteQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let channel_id = match parse_channel_id(&raw_id) {
        Ok(channel_id) => channel_id,
        Err(error) => return management_error(error),
    };
    let query = match query {
        Ok(Query(query)) => query,
        Err(_error) => {
            return no_store(problem(
                StatusCode::BAD_REQUEST,
                ProblemCode::InvalidRequest,
                "request does not match the channel schema",
            ))
        }
    };
    let Some(expected_version) = query.expected_version else {
        return no_store(problem(
            StatusCode::BAD_REQUEST,
            ProblemCode::InvalidRequest,
            "expected_version is required",
        ));
    };
    if query.retire_unready {
        return match management::retire_unready(&state, &creator, &channel_id, expected_version)
            .await
        {
            Ok(()) => no_store(StatusCode::ACCEPTED.into_response()),
            Err(error) => management_error(error),
        };
    }
    match management::delete(&state, &creator, &channel_id, expected_version).await {
        Ok(()) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => management_error(error),
    }
}

#[utoipa::path(
    post,
    path = "/v1/channels/{channel_id}/test",
    operation_id = "testChannel",
    tag = "channels",
    security(("creatorBearer" = [])),
    params(("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), ("channel_id" = String, Path)),
    responses((status = 200, body = TestChannelResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn test_channel(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw_id): Path<String>,
) -> Response {
    let operation = AuthorizedChannelTest { state, creator };
    with_management_failure_stage(
        cmaeg_http::http::management::test_channel(&operation, raw_id).await,
    )
}

struct AuthorizedChannelTest {
    state: ManagementState,
    creator: Creator,
}

impl ChannelTest for AuthorizedChannelTest {
    fn test_channel(
        &self,
        channel_id: ChannelId,
    ) -> Pin<Box<dyn Future<Output = Result<TestChannelResponse, ManagementFailure>> + Send + '_>>
    {
        Box::pin(async move {
            let result = management::test_turn(&self.state, &self.creator, &channel_id)
                .await
                .map_err(management_failure)?;
            Ok(TestChannelResponse {
                accepted_after_ms: result.accepted_after_ms,
                first_text: result.first_text,
                level: result.level,
            })
        })
    }
}

#[utoipa::path(
    post,
    path = "/v1/channels/{channel_id}/rekey",
    operation_id = "rekeyChannel",
    tag = "channels",
    security(("serviceBearer" = [])),
    params(("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), ("channel_id" = String, Path)),
    responses((status = 200, body = RekeyResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json"))
)]
pub async fn rekey_channel(
    State(state): State<ManagementState>,
    Path(raw_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !service_authorized(&headers, state.cma_service_token.as_str()) {
        return no_store(problem(
            StatusCode::UNAUTHORIZED,
            ProblemCode::NotAuthenticated,
            "CMA service authentication is required",
        ));
    }
    let channel_id = match parse_channel_id(&raw_id) {
        Ok(channel_id) => channel_id,
        Err(error) => return management_error(error),
    };
    match management::rekey(&state, &channel_id).await {
        Ok(kid) => no_store(Json(RekeyResponse { kid }).into_response()),
        Err(error) => management_error(error),
    }
}

#[derive(Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in=Query)]
pub struct ListQuery {
    pub after: Option<String>,
}
/// An owner-visible record whose original channel ID may predate recoverable creation.
#[derive(Serialize, ToSchema)]
pub struct ChannelListItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_discord_activation: Option<crate::discord::activation::NativeDiscordActivation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    pub record_id: String,
    pub version: u64,
    pub provider: ProviderResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<EndpointResponse>,
    pub definition: super::schema::PublicChannelDefinition,
}

#[derive(Serialize, ToSchema)]
pub struct ChannelPage {
    pub items: Vec<ChannelListItem>,
    pub next_cursor: Option<String>,
}
#[utoipa::path(get,path="/v1/channels",operation_id="listChannels",tag="channels",security(("creatorBearer"=[])),params(ListQuery,("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator")),responses((status=200,body=ChannelPage),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn list_channels(
    State(state): State<ManagementState>,
    creator: Creator,
    Query(query): Query<ListQuery>,
) -> Response {
    let heads = match state
        .records
        .list(Some(&creator.subject), query.after.as_deref(), 51)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return management_error(ManagementError::Record(error)),
    };
    let next_cursor = if heads.len() > 50 {
        heads.get(49).map(|h| h.record_id.clone())
    } else {
        None
    };
    let mut items = Vec::new();
    for head in heads.into_iter().take(50) {
        let id = match cmaeg_core::ids::RecordId::new(head.record_id) {
            Ok(id) => id,
            Err(e) => return management_error(ManagementError::Invalid(e)),
        };
        let loaded = match management::load_by_record(&state, &creator, &id).await {
            Ok(value) => value,
            Err(error) => return management_error(error),
        };
        match channel_list_item(&state, &loaded) {
            Ok(value) => items.push(value),
            Err(error) => return management_error(error),
        }
    }
    no_store(Json(ChannelPage { items, next_cursor }).into_response())
}

#[utoipa::path(get, path="/v1/permissions", operation_id="getPermissions", tag="identity",
    security(("creatorBearer"=[])), responses((status=200, body=serde_json::Value),
    (status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn permission_catalog(creator: Creator) -> Json<serde_json::Value> {
    Json(serde_json::json!({"subject":creator.subject,"roles":crate::auth::permissions::ROLES}))
}
