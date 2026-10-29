//! Body-carried read-only event capability. Never put a sealed reference in a
//! URL, response, log, or redirect, and never expose the retained event itself.
use super::tools::{json, result};
use crate::{
    tools::ToolsState,
    turns::routed::{EventAttestationRequest, EventAttestationResponse, ExecutionError},
};
use axum::{
    extract::{rejection::JsonRejection, State},
    http::{header::CACHE_CONTROL, HeaderValue},
    response::Response,
    routing::post,
    Json, Router,
};
use cmaeg_core::problem::{Problem, ProblemCode};
use std::time::Duration;
use tower_http::limit::RequestBodyLimitLayer;

pub(crate) fn routes() -> Router<ToolsState> {
    Router::new()
        .route("/v1/event-attestations", post(attest_event))
        .layer(RequestBodyLimitLayer::new(16 * 1024))
}

#[utoipa::path(post,path="/v1/event-attestations",operation_id="attestRetainedEvent",tag="provider integration",
    description="Read-only exact-event capability. The sealed event_ref in the request body authorizes only a boolean current-source/current-target check. No event content, credentials or workspace paths are returned. Agent Manager must separately validate the original admitted command and its actual binding generation. This endpoint never starts a response or changes the original execution lease.",
    request_body=EventAttestationRequest,
    responses((status=200,body=EventAttestationResponse),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn attest_event(
    State(state): State<ToolsState>,
    body: Result<Json<EventAttestationRequest>, JsonRejection>,
) -> Response {
    let outcome = async {
        let request = json(body)?;
        let routed = state.routed_replies.as_ref().ok_or_else(unavailable)?;
        let current =
            match tokio::time::timeout(Duration::from_secs(5), routed.attest_event(&request)).await
            {
                Ok(Ok(current)) => current,
                Ok(Err(
                    ExecutionError::Invalid
                    | ExecutionError::Expired
                    | ExecutionError::AuthorityChanged,
                )) => false,
                _ => return Err(unavailable()),
            };
        Ok(EventAttestationResponse { current })
    }
    .await;
    let mut response = result(outcome);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn unavailable() -> Problem {
    Problem::new(ProblemCode::Internal, 503, "event_attestation_unavailable")
}
