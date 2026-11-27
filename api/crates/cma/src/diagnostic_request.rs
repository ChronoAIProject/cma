pub(crate) use cma_am_auth::diagnostic_request::*;

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue},
    middleware::{from_fn, Next},
    response::Response,
    Router,
};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

pub(super) fn with_request_context(router: Router) -> Router {
    router
        .layer(from_fn(crate::telemetry::http::observe))
        .layer(PropagateRequestIdLayer::new(X_REQUEST_ID))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(request_span)
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(from_fn(scope))
        .layer(SetRequestIdLayer::new(X_REQUEST_ID, MakeRequestUuid))
        .layer(from_fn(discard_unusable_request_id))
}

async fn discard_unusable_request_id(mut request: Request, next: Next) -> Response {
    let normalized = layer1_public::rest::request_id_from_headers(request.headers())
        .and_then(|value| HeaderValue::from_bytes(value.as_bytes()).ok());
    request.headers_mut().remove(X_REQUEST_ID);
    if let Some(normalized) = normalized {
        request.headers_mut().insert(X_REQUEST_ID, normalized);
    }
    next.run(request).await
}

#[cfg(test)]
#[path = "diagnostic_request_tests.rs"]
mod tests;
