//! `cmaeg_console_requests_total{page, outcome}`: every console request,
//! counted by its route template and a bounded outcome. The page label is
//! the matched template, never the request path, so record ids, event keys
//! and source ids never become label values.
use axum::{
    extract::{MatchedPath, Request},
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use crate::telemetry::names;

/// Count `request` once its response is known.
pub(crate) async fn count(request: Request, next: Next) -> Response {
    let page = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |path| path.as_str().to_owned());
    let response = next.run(request).await;
    metrics::counter!(
        names::CONSOLE_REQUESTS,
        names::PAGE => page,
        names::OUTCOME => outcome(response.status()),
    )
    .increment(1);
    response
}

fn outcome(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "invalid",
        StatusCode::FORBIDDEN => "refused",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::CONFLICT => "conflict",
        StatusCode::TOO_MANY_REQUESTS => "rate_limited",
        StatusCode::NOT_IMPLEMENTED => "not_available",
        StatusCode::SERVICE_UNAVAILABLE => "unavailable",
        status if status.is_success() => "ok",
        status if status.is_redirection() => "redirect",
        status if status.is_client_error() => "client_error",
        _ => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_bounded_outcomes() {
        for (status, expected) in [
            (200, "ok"),
            (303, "redirect"),
            (400, "invalid"),
            (403, "refused"),
            (404, "not_found"),
            (409, "conflict"),
            (429, "rate_limited"),
            (501, "not_available"),
            (503, "unavailable"),
            (418, "client_error"),
            (500, "error"),
        ] {
            assert_eq!(outcome(StatusCode::from_u16(status).unwrap()), expected);
        }
    }
}
