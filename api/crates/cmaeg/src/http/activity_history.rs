//! Every page uses the current read grant and channel ownership. A cursor is
//! only a sealed query continuation, never an authorization capability.
use super::{
    activity::owned,
    management_responses::{management_error, parse_channel_id},
    problems::no_store,
};
use crate::{
    activity::history::{DetailsQuery, Error, HistoryDetails, HistoryPage, HistoryQuery},
    auth::Creator,
    channels::management::ManagementState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

#[utoipa::path(get,path="/v1/channels/{channel_id}/activity/history",operation_id="getChannelActivityHistory",tag="channels",
security(("creatorBearer"=[])),params(HistoryQuery,("channel_id"=String,Path),("X-CMAEG-Owner"=Option<String>,Header)),
responses((status=200,body=HistoryPage),(status=400,description="Invalid range or cursor"),(status=410,description="Paging session expired"),(status=503,description="History unavailable"),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn history(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw): Path<String>,
    query: Result<Query<HistoryQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let id = match parse_channel_id(&raw) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    let channel = match owned(&state, &creator, &id).await {
        Ok(c) => c,
        Err(error) => return management_error(error),
    };
    let Query(query) = match query {
        Ok(q) => q,
        Err(_) => return failure(Error::InvalidRequest),
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.activity.history.page(
            &creator.subject,
            channel.definition.record_id.as_str(),
            query,
        ),
    )
    .await
    {
        Ok(Ok(page)) => no_store(Json(page).into_response()),
        Ok(Err(error)) => failure(error),
        Err(_) => failure(Error::Unavailable),
    }
}

#[utoipa::path(get,path="/v1/channels/{channel_id}/activity/history/events/{event_id}/details",operation_id="getChannelActivityHistoryDetails",tag="channels",
security(("creatorBearer"=[])),params(DetailsQuery,("channel_id"=String,Path),("event_id"=String,Path),("X-CMAEG-Owner"=Option<String>,Header)),
responses((status=200,body=HistoryDetails),(status=400,description="Invalid detail cursor"),(status=410,description="Paging session expired"),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn details(
    State(state): State<ManagementState>,
    creator: Creator,
    Path((raw, event)): Path<(String, String)>,
    query: Result<Query<DetailsQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let id = match parse_channel_id(&raw) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    let channel = match owned(&state, &creator, &id).await {
        Ok(c) => c,
        Err(error) => return management_error(error),
    };
    let Query(query) = match query {
        Ok(q) => q,
        Err(_) => return failure(Error::InvalidRequest),
    };
    let Some(token) = query.cursor.as_deref() else {
        return failure(Error::InvalidRequest);
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.activity.history.details(
            &creator.subject,
            channel.definition.record_id.as_str(),
            &event,
            token,
        ),
    )
    .await
    {
        Ok(Ok(page)) => no_store(Json(page).into_response()),
        Ok(Err(error)) => failure(error),
        Err(_) => failure(Error::Unavailable),
    }
}
fn failure(error: Error) -> Response {
    let (status, code) = match error {
        Error::InvalidRequest | Error::InvalidConfiguration => {
            (StatusCode::BAD_REQUEST, "invalid_activity_history_request")
        }
        Error::Expired => (StatusCode::GONE, "activity_history_cursor_expired"),
        Error::NotFound => (StatusCode::NOT_FOUND, "activity_history_not_found"),
        Error::UnsupportedBackend => (StatusCode::NOT_IMPLEMENTED, "unsupported_backend"),
        Error::Disabled => (StatusCode::SERVICE_UNAVAILABLE, "activity_history_disabled"),
        Error::Unreadable => (StatusCode::SERVICE_UNAVAILABLE, "unreadable_record"),
        Error::DetailLimit => (
            StatusCode::SERVICE_UNAVAILABLE,
            "activity_history_detail_limit",
        ),
        Error::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "activity_history_unavailable",
        ),
    };
    // Only a closed set of reason codes reaches logs/metrics; Mongo/serde
    // errors may contain sensitive metadata and are deliberately not formatted.
    metrics::counter!("cmaeg_activity_history_errors_total", "reason" => code).increment(1);
    if status.is_server_error() {
        tracing::warn!(reason = code, "activity history request unavailable");
    }
    no_store((status, [("content-type", "application/problem+json")], Json(serde_json::json!({
        "type":format!("urn:cmaeg:problem:{code}"),"title":code,"status":status.as_u16(),"code":code,"detail":error.to_string()
    }))).into_response())
}
