//! Operator transport for the configured staging suggestion fixture.
use axum::{
    extract::{rejection::JsonRejection, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use layer1_public::AuthenticatedCaller;
use layer2_core::{Caller, Capability};
use serde::Deserialize;

use crate::{config::suggestion_control::Mode, store::suggestion_control::SuggestionControl};

const PATH: &str = "/api/v1/admin/staging/suggestion-control";
const RELEASE_PATH: &str = "/api/v1/admin/staging/suggestion-control/release";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arm {
    mode: Mode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Release {
    claim_started_at_ms: i64,
}

pub(super) fn routes(router: Router, control: Option<SuggestionControl>) -> Router {
    let Some(control) = control else {
        return router;
    };
    router.merge(
        Router::new()
            .route(PATH, get(read).post(arm))
            .route(RELEASE_PATH, post(release))
            .with_state(control),
    )
}

fn authorize(caller: &Caller) -> Result<(), StatusCode> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| StatusCode::FORBIDDEN)
}

async fn read(
    State(control): State<SuggestionControl>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<impl IntoResponse, StatusCode> {
    authorize(&caller)?;
    let report = control
        .read()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(report)))
}

async fn arm(
    State(control): State<SuggestionControl>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<Arm>, JsonRejection>,
) -> Result<StatusCode, StatusCode> {
    authorize(&caller)?;
    let Json(body) = body.map_err(|_| StatusCode::BAD_REQUEST)?;
    accepted(control.arm(body.mode).await)
}

async fn release(
    State(control): State<SuggestionControl>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<Release>, JsonRejection>,
) -> Result<StatusCode, StatusCode> {
    authorize(&caller)?;
    let Json(body) = body.map_err(|_| StatusCode::BAD_REQUEST)?;
    accepted(control.release(body.claim_started_at_ms).await)
}

fn accepted(
    result: Result<bool, layer2_core::sandbox::AgentReadError>,
) -> Result<StatusCode, StatusCode> {
    if result.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::CONFLICT)
    }
}

#[cfg(test)]
mod tests;
