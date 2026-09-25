//! Expiring opaque gateway-state transport. No interaction payload is interpreted here.
use super::gateway_records::{GatewayRecordView, PutGatewayRecordView};
use crate::AuthenticatedCaller;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::put,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use layer2_core::{gateway_interactions::GatewayInteractionService, CoreError, SubjectId};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct PutInteraction {
    owner: String,
    blob: String,
    expected_version: Option<u64>,
    expires_at: i64,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct DeleteInteraction {
    expected_version: u64,
}

pub fn gateway_interaction_router(service: Arc<GatewayInteractionService>) -> Router {
    Router::new()
        .route(
            "/api/v1/gateway/interaction-records/{record_id}",
            put(write).get(read).delete(delete),
        )
        .with_state(service)
        .layer(axum::middleware::from_fn(
            |request: axum::extract::Request, next: axum::middleware::Next| async move {
                (
                    [(header::CACHE_CONTROL, "no-store")],
                    next.run(request).await,
                )
                    .into_response()
            },
        ))
}

#[utoipa::path(put,path="/api/v1/gateway/interaction-records/{record_id}",tag="event gateway",params(("record_id"=String,Path)),request_body=PutInteraction,responses((status=200,body=PutGatewayRecordView),(status=400),(status=403),(status=409)))]
async fn write(
    State(service): State<Arc<GatewayInteractionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    Json(body): Json<PutInteraction>,
) -> Response {
    let blob = match STANDARD.decode(&body.blob) {
        Ok(blob) => blob,
        Err(_) => return failure(CoreError::InvalidRequest("blob must be base64".into())),
    };
    match service
        .put(
            &caller,
            &id,
            &SubjectId::new(body.owner),
            blob,
            body.expected_version,
            body.expires_at,
        )
        .await
    {
        Ok(record) => Json(PutGatewayRecordView::from(record)).into_response(),
        Err(error) => failure(error),
    }
}

#[utoipa::path(get,path="/api/v1/gateway/interaction-records/{record_id}",tag="event gateway",params(("record_id"=String,Path)),responses((status=200,body=GatewayRecordView),(status=403),(status=404)))]
async fn read(
    State(service): State<Arc<GatewayInteractionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    match service.get(&caller, &id).await {
        Ok(Some(record)) => Json(GatewayRecordView::from(record)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => failure(error),
    }
}

#[utoipa::path(delete,path="/api/v1/gateway/interaction-records/{record_id}",tag="event gateway",params(("record_id"=String,Path),DeleteInteraction),responses((status=204),(status=403),(status=404),(status=409)))]
async fn delete(
    State(service): State<Arc<GatewayInteractionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    Query(query): Query<DeleteInteraction>,
) -> Response {
    match service.delete(&caller, &id, query.expected_version).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => failure(error),
    }
}

fn failure(error: CoreError) -> Response {
    super::gateway_records::gateway_error(error)
}
