//! Write admission uses the same durable head as freeze and provider dispatch.
use super::Session;
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use layer3_adapter::effect_authority::{EffectAuthority, EffectIntent, ObservedOutcome};
use std::sync::Arc;

pub(crate) fn guard(router: axum::Router, session: Option<Arc<Session>>) -> axum::Router {
    match session {
        Some(session) => router.layer(axum::middleware::from_fn_with_state(session, admit)),
        None => router,
    }
}

async fn admit(State(session): State<Arc<Session>>, request: Request, next: Next) -> Response {
    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let Ok(body) = to_bytes(body, crate::REQUEST_BODY_LIMIT_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let identity = format!("{}:{}", parts.method, parts.uri.path());
    let payload = (
        &identity,
        parts.uri.to_string(),
        body.as_ref(),
        parts
            .headers
            .get(header::AUTHORIZATION)
            .map(|value| value.as_bytes()),
        parts
            .headers
            .get(header::COOKIE)
            .map(|value| value.as_bytes()),
    );
    let Ok(intent) = EffectIntent::new("http_mutation", &identity, &payload) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(permit) = session.admit(&intent).await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let response = next.run(Request::from_parts(parts, Body::from(body))).await;
    let outcome = if response.status().is_success() {
        ObservedOutcome::Acknowledged
    } else {
        ObservedOutcome::Unknown
    };
    if permit.record_outcome(outcome).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    response
}
