//! Owner-scoped recovery of a reserved channel after a lost create response.
use super::{
    management::{ChannelListItem, ChannelResponse},
    management_responses::{
        channel_list_item, channel_response, management_error, provider_response,
    },
    problems::no_store,
};
use crate::{
    auth::Creator,
    channels::{
        creation,
        management::{load_by_record, ManagementError, ManagementState},
    },
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::{ids::RecordId, ValidationError};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RecoveryQuery {
    pub include_legacy: Option<bool>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum ManagementHandle {
    Missing,
}

#[derive(Serialize, ToSchema)]
pub struct LegacyChannelRecord {
    management_handle: ManagementHandle,
    #[serde(flatten)]
    record: ChannelListItem,
}

#[derive(Serialize, ToSchema)]
#[serde(untagged)]
pub enum ChannelRecordResponse {
    Recoverable(ChannelResponse),
    Legacy(LegacyChannelRecord),
}

fn recovery_record_id(id: String) -> Result<RecordId, ValidationError> {
    if !uuid::Uuid::parse_str(&id)
        .is_ok_and(|parsed| parsed.get_version_num() == 4 && parsed.to_string() == id)
    {
        return Err(ValidationError::new(
            "/record_id",
            "must be a canonical UUID v4",
        ));
    }
    RecordId::new(id)
}

#[utoipa::path(get, path="/v1/channel-records/{record_id}", operation_id="recoverChannel", tag="channels",
    security(("creatorBearer"=[])), params(("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), ("record_id"=String, Path), RecoveryQuery),
    responses((status=200, body=ChannelRecordResponse), (status="default", body=cmaeg_core::problem::Problem, content_type="application/problem+json")))]
pub async fn recover_channel(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(id): Path<String>,
    query: Result<Query<RecoveryQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let id = match recovery_record_id(id) {
        Ok(id) => id,
        Err(error) => return management_error(ManagementError::Invalid(error)),
    };
    let include_legacy = if state.legacy_channel_record_read_enabled {
        match query {
            Ok(Query(query)) => query.include_legacy.unwrap_or_default(),
            Err(_) => {
                return management_error(ManagementError::Invalid(ValidationError::new(
                    "/include_legacy",
                    "must be a boolean",
                )));
            }
        }
    } else {
        false
    };
    let current = match creation::load_record(&state, &creator, &id).await {
        Ok(value) => value,
        Err(ManagementError::NotFound) if include_legacy => {
            return match read_legacy(&state, &creator, &id).await {
                Ok(value) => no_store(Json(ChannelRecordResponse::Legacy(value)).into_response()),
                Err(error) => management_error(error),
            };
        }
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
        Ok(value) => no_store(
            (
                StatusCode::OK,
                Json(ChannelRecordResponse::Recoverable(value)),
            )
                .into_response(),
        ),
        Err(error) => management_error(error),
    }
}

async fn read_legacy(
    state: &ManagementState,
    creator: &Creator,
    id: &RecordId,
) -> Result<LegacyChannelRecord, ManagementError> {
    let recorded = load_by_record(state, creator, id).await?;
    if recorded.definition.creation.is_some() {
        return Err(ManagementError::NotFound);
    }
    Ok(LegacyChannelRecord {
        management_handle: ManagementHandle::Missing,
        record: channel_list_item(state, &recorded)?,
    })
}
