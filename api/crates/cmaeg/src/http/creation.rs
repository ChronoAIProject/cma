//! Owner-scoped recovery of a reserved channel after a lost create response.
use super::record_ids::canonical_record_id;
use super::{
    management::ChannelResponse,
    management_responses::{channel_response, management_error, provider_response},
    problems::{no_store, problem},
};
use crate::{
    auth::Creator,
    channels::{
        creation,
        management::{ManagementError, ManagementState},
    },
    records::CancelCreate,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::problem::ProblemCode;
use serde::Serialize;
use utoipa::ToSchema;

#[utoipa::path(get, path="/v1/channel-records/{record_id}", operation_id="recoverChannel", tag="channels",
    security(("creatorBearer"=[])), params(("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), ("record_id"=String, Path)),
    responses((status=200, body=ChannelResponse), (status="default", body=cmaeg_core::problem::Problem, content_type="application/problem+json")))]
pub async fn recover_channel(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(id): Path<String>,
) -> Response {
    let id = match canonical_record_id(id) {
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

const CANCEL_CREATE_VERSION: u16 = 1;

#[derive(Serialize, ToSchema)]
pub(crate) struct CancelCreateCapability {
    contract_version: u16,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct CancelCreateReceipt {
    contract_version: u16,
    status: &'static str,
    record_id: String,
    owner: CancelCreateReceiptOwner,
    provider: CancelCreateReceiptProvider,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct CancelCreateReceiptOwner {
    issuer: String,
    kind: &'static str,
    subject: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct CancelCreateReceiptProvider {
    slug: &'static str,
    binding_id: String,
}

#[utoipa::path(get, path="/v1/channel-records/cancel-create-capability", operation_id="cancelCreateCapability", tag="channels",
    security(("creatorBearer"=[])),
    responses((status=200, body=CancelCreateCapability), (status="default", body=cmaeg_core::problem::Problem, content_type="application/problem+json")))]
pub(crate) async fn cancel_create_capability(
    State(state): State<ManagementState>,
    headers: HeaderMap,
) -> Response {
    if !state.records.channel_cancellation_enabled() {
        return cancellation_unavailable();
    }
    let owner = match state
        .creator_auth
        .authenticate_create_cancellation(&headers)
        .await
    {
        Ok(owner) => owner,
        Err(error) => return no_store(error.into_response()),
    };
    if !matches!(
        state.records.channel_create_capability(&owner.issuer).await,
        Ok(true)
    ) {
        return cancellation_unavailable();
    }
    no_store(
        Json(CancelCreateCapability {
            contract_version: CANCEL_CREATE_VERSION,
        })
        .into_response(),
    )
}

#[utoipa::path(post, path="/v1/channel-records/{record_id}/cancel-create", operation_id="cancelChannelCreate", tag="channels",
    security(("creatorBearer"=[])), params(("record_id"=String, Path)),
    responses((status=200, body=CancelCreateReceipt), (status="default", body=cmaeg_core::problem::Problem, content_type="application/problem+json")))]
pub(crate) async fn cancel_create(
    State(state): State<ManagementState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !state.records.channel_cancellation_enabled() {
        return cancellation_unavailable();
    }
    let owner = match state
        .creator_auth
        .authenticate_create_cancellation(&headers)
        .await
    {
        Ok(owner) => owner,
        Err(error) => return no_store(error.into_response()),
    };
    let record_id = match canonical_record_id(id) {
        Ok(record_id) => record_id,
        Err(error) => return management_error(ManagementError::Invalid(error)),
    };
    match state
        .records
        .cancel_channel_create(&record_id, &owner, &state.master_keys)
        .await
    {
        Ok(CancelCreate::FencedAbsent) => no_store(
            Json(CancelCreateReceipt {
                contract_version: CANCEL_CREATE_VERSION,
                status: "creation_fenced_absent",
                record_id: record_id.to_string(),
                owner: CancelCreateReceiptOwner {
                    issuer: owner.issuer,
                    kind: "human",
                    subject: owner.subject,
                },
                provider: CancelCreateReceiptProvider {
                    slug: "nyxbot",
                    binding_id: format!("bnd_{}", record_id.as_str().replace('-', "")),
                },
            })
            .into_response(),
        ),
        Ok(CancelCreate::RecordPresent) => no_store(problem(
            StatusCode::CONFLICT,
            ProblemCode::Conflict,
            "record_present",
        )),
        Ok(CancelCreate::CreatePending) => no_store(problem(
            StatusCode::CONFLICT,
            ProblemCode::Conflict,
            "create_pending",
        )),
        Err(error) => management_error(ManagementError::Record(error)),
    }
}

fn cancellation_unavailable() -> Response {
    no_store(problem(
        StatusCode::SERVICE_UNAVAILABLE,
        ProblemCode::ProviderUnavailable,
        "channel_create_cancellation_unavailable",
    ))
}
