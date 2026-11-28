pub(crate) use cma_am_auth::diagnostic_request::*;

use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};

pub(crate) async fn discard_unusable_request_id(mut request: Request, next: Next) -> Response {
    use super::X_REQUEST_ID;

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
