//! Authenticated management disposition; never an unauthenticated repair hook.
use super::{
    management_responses::{management_error, parse_channel_id},
    problems::{no_store, problem},
};
use crate::{
    auth::{Creator, PartitionReleaseActor},
    channels::management::{self, ManagementState},
    turns::routed::{
        unresolved::{HeadPreview, ReleaseRequest, ReleaseResult},
        ExecutionError,
    },
};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::problem::ProblemCode;

#[utoipa::path(get, path = "/v1/channels/{channel_id}/partitions/{partition_id}/head",
    operation_id = "inspectUnresolvedPartitionHead", tag = "channels", security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("partition_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header)),
    responses((status = 200, body = HeadPreview), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json")))]
pub async fn inspect(
    State(state): State<ManagementState>,
    creator: Creator,
    Path((channel, partition)): Path<(String, String)>,
) -> Response {
    let channel = match parse_channel_id(&channel) {
        Ok(value) => value,
        Err(error) => return management_error(error),
    };
    if let Err(error) = management::load(&state, &creator, &channel).await {
        return management_error(error);
    }
    let Some(service) = &state.unresolved_release else {
        return failure(ExecutionError::Unsupported);
    };
    match service
        .preview(&channel, &creator.subject, &partition)
        .await
    {
        Ok(value) => no_store(Json(value).into_response()),
        Err(error) => failure(error),
    }
}

/// Owner credentials, or a verified organization administrator selecting that
/// owner, may explicitly abandon an expired unknown. The request attests that
/// the provider is terminal; this endpoint never treats elapsed time as proof.
#[utoipa::path(post, path = "/v1/channels/{channel_id}/partitions/{partition_id}/release",
    operation_id = "releaseUnresolvedPartitionHead", tag = "channels", security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("partition_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header)),
    request_body = ReleaseRequest,
    responses((status = 200, body = ReleaseResult), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json")))]
pub async fn release(
    State(state): State<ManagementState>,
    actor: PartitionReleaseActor,
    Path((channel, partition)): Path<(String, String)>,
    body: Result<Json<ReleaseRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let creator = actor.creator;
    let channel = match parse_channel_id(&channel) {
        Ok(value) => value,
        Err(error) => return management_error(error),
    };
    if let Err(error) = management::load(&state, &creator, &channel).await {
        return management_error(error);
    }
    let request = match body {
        Ok(Json(value)) => value,
        Err(_) => return failure(ExecutionError::Invalid),
    };
    if request.partition_id != partition {
        return failure(ExecutionError::Invalid);
    }
    let Some(service) = &state.unresolved_release else {
        return failure(ExecutionError::Unsupported);
    };
    match service
        .release_as(
            &channel,
            &creator.subject,
            &actor.actor,
            &creator.token_hash,
            &request,
        )
        .await
    {
        Ok(value) => no_store(Json(value).into_response()),
        Err(error) => failure(error),
    }
}

fn failure(error: ExecutionError) -> Response {
    let (status, code, detail) = match error {
        ExecutionError::Invalid => (
            400,
            ProblemCode::InvalidRequest,
            "invalid_unresolved_release",
        ),
        ExecutionError::AuthorityChanged => {
            (404, ProblemCode::ChannelNotFound, "partition_not_found")
        }
        ExecutionError::Busy | ExecutionError::LostLease | ExecutionError::IdempotencyConflict => {
            (409, ProblemCode::Conflict, "partition_head_changed_or_busy")
        }
        ExecutionError::Expired => (
            409,
            ProblemCode::Conflict,
            "unresolved_release_outside_bound",
        ),
        ExecutionError::Unsupported => (409, ProblemCode::Conflict, "unresolved_release_disabled"),
        _ => (503, ProblemCode::Internal, "unresolved_release_unavailable"),
    };
    no_store(problem(
        axum::http::StatusCode::from_u16(status).expect("constant status"),
        code,
        detail,
    ))
}
