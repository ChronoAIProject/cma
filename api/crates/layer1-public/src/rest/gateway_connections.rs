//! Opaque connection records. Platform meaning and credentials belong to the gateway.
use super::gateway_records::*;
use crate::AuthenticatedCaller;
use axum::{
    extract::{Path, Query, State},
    response::Response,
    routing::{get, put},
    Json, Router,
};
use layer2_core::GatewayRecordService;
use std::sync::Arc;

pub fn gateway_connection_record_router(service: Arc<GatewayRecordService>) -> Router {
    Router::new()
        .route("/api/v1/gateway/connection-records", get(list))
        .route(
            "/api/v1/gateway/connection-records/{record_id}",
            put(write).get(read).delete(remove),
        )
        .with_state(service)
        .layer(axum::middleware::from_fn(
            super::gateway_records::add_no_store,
        ))
}

#[utoipa::path(put,path="/api/v1/gateway/connection-records/{record_id}",tag="event gateway",params(("record_id"=String,Path)),request_body=PutGatewayRecordRequest,responses((status=200,body=PutGatewayRecordView),(status=400),(status=403),(status=409)))]
async fn write(
    state: State<Arc<GatewayRecordService>>,
    caller: AuthenticatedCaller,
    id: Path<String>,
    body: Result<Json<PutGatewayRecordRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    super::gateway_records::put_record(state, caller, id, body).await
}

#[utoipa::path(get,path="/api/v1/gateway/connection-records/{record_id}",tag="event gateway",params(("record_id"=String,Path)),responses((status=200,body=GatewayRecordView),(status=403),(status=404)))]
async fn read(
    state: State<Arc<GatewayRecordService>>,
    caller: AuthenticatedCaller,
    id: Path<String>,
) -> Response {
    super::gateway_records::get_record(state, caller, id).await
}

#[utoipa::path(delete,path="/api/v1/gateway/connection-records/{record_id}",tag="event gateway",params(("record_id"=String,Path),GatewayRecordDeleteQuery),responses((status=204),(status=403),(status=404),(status=409)))]
async fn remove(
    state: State<Arc<GatewayRecordService>>,
    caller: AuthenticatedCaller,
    id: Path<String>,
    query: Result<Query<GatewayRecordDeleteQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    super::gateway_records::delete_record(state, caller, id, query).await
}

#[utoipa::path(get,path="/api/v1/gateway/connection-records",tag="event gateway",params(GatewayRecordListQuery),responses((status=200,body=GatewayRecordPageView),(status=403)))]
async fn list(
    state: State<Arc<GatewayRecordService>>,
    caller: AuthenticatedCaller,
    query: Result<Query<GatewayRecordListQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    super::gateway_records::list(state, caller, query).await
}
