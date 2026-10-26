//! Public owner delivery projections; all authority and effects live in outbound.
use crate::{
    auth::Creator,
    channels::state::ManagementState,
    outbound::{self, OutboundReceipt, PrepareOutboundMessage},
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use cmaeg_core::{
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
};

pub fn routes() -> Router<ManagementState> {
    Router::new()
        .route("/v1/channels/{channel_id}/outbound-messages", post(prepare))
        .route(
            "/v1/channels/{channel_id}/outbound-messages/{delivery_id}",
            get(read).delete(cancel),
        )
        .route(
            "/v1/channels/{channel_id}/outbound-messages/{delivery_id}/send",
            post(commit),
        )
        .layer(tower_http::limit::RequestBodyLimitLayer::new(131072))
}
fn response(result: Result<OutboundReceipt, Problem>, status: StatusCode) -> Response {
    super::problems::no_store(match result {
        Ok(value) => (status, Json(value)).into_response(),
        Err(error) => super::problems::problem(
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            error.code,
            error.detail.unwrap_or_default(),
        ),
    })
}
fn json<T>(value: Result<Json<T>, axum::extract::rejection::JsonRejection>) -> Result<T, Problem> {
    value
        .map(|Json(body)| body)
        .map_err(|_| Problem::new(ProblemCode::InvalidRequest, 400, "outbound_request_invalid"))
}
#[utoipa::path(post,path="/v1/channels/{channel_id}/outbound-messages",operation_id="prepareOutboundMessage",tag="outbound messages",security(("creatorBearer"=[])),params(("channel_id"=String,Path),("Idempotency-Key"=String,Header)),request_body=PrepareOutboundMessage,responses((status=201,body=OutboundReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn prepare(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<PrepareOutboundMessage>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let result = async {
        let body = json(body)?;
        let mut values = headers.get_all("idempotency-key").iter();
        let key = values
            .next()
            .and_then(|value| value.to_str().ok())
            .filter(|_| values.next().is_none())
            .ok_or_else(|| {
                Problem::new(
                    ProblemCode::InvalidRequest,
                    400,
                    "one_idempotency_key_is_required",
                )
            })?;
        outbound::prepare(&state, &creator, &id, key, body).await
    }
    .await;
    response(result, StatusCode::CREATED)
}
#[utoipa::path(get,path="/v1/channels/{channel_id}/outbound-messages/{delivery_id}",operation_id="getOutboundMessage",tag="outbound messages",security(("creatorBearer"=[])),params(("channel_id"=String,Path),("delivery_id"=String,Path)),responses((status=200,body=OutboundReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn read(
    State(state): State<ManagementState>,
    creator: Creator,
    Path((channel, id)): Path<(String, String)>,
) -> Response {
    response(
        outbound::read(&state, &creator, &channel, &id).await,
        StatusCode::OK,
    )
}
#[utoipa::path(delete,path="/v1/channels/{channel_id}/outbound-messages/{delivery_id}",operation_id="cancelOutboundMessage",tag="outbound messages",security(("creatorBearer"=[])),params(("channel_id"=String,Path),("delivery_id"=String,Path)),responses((status=200,body=OutboundReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn cancel(
    State(state): State<ManagementState>,
    creator: Creator,
    Path((channel, id)): Path<(String, String)>,
) -> Response {
    response(
        outbound::cancel(&state, &creator, &channel, &id).await,
        StatusCode::OK,
    )
}
#[utoipa::path(post,path="/v1/channels/{channel_id}/outbound-messages/{delivery_id}/send",operation_id="commitOutboundMessage",tag="outbound messages",security(("creatorBearer"=[])),params(("channel_id"=String,Path),("delivery_id"=String,Path)),request_body=OutboundMessage,responses((status=200,body=OutboundReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn commit(
    State(state): State<ManagementState>,
    creator: Creator,
    Path((channel, id)): Path<(String, String)>,
    body: Result<Json<OutboundMessage>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let result =
        async { outbound::commit(&state, &creator, &channel, &id, json(body)?).await }.await;
    response(result, StatusCode::OK)
}
