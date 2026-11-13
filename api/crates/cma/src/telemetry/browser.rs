//! A bounded, anonymous diagnostic receiver, independent of auth and Mongo.
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use governor::{Quota, RateLimiter};
use layer1_public::rest::BrowserFault;
use std::{num::NonZeroU32, sync::Arc};
type Budget = governor::DefaultDirectRateLimiter;
pub(crate) fn initialize() {
    for kind in [
        "uncaught_error",
        "unhandled_rejection",
        "react_error",
        "resource_error",
        "chunk_error",
    ] {
        metrics::counter!("cma_browser_fault_reports_total", "kind" => kind).absolute(0);
    }
    metrics::counter!("cma_browser_fault_reports_limited_total").absolute(0);
}
pub(crate) fn router() -> Router {
    let budget = Arc::new(RateLimiter::direct(Quota::per_second(
        NonZeroU32::new(20).unwrap(),
    )));
    layer1_public::rest::browser_telemetry_router(|fault: BrowserFault| {
        metrics::counter!("cma_browser_fault_reports_total", "kind" => fault.as_str()).increment(1);
    })
    .layer(middleware::from_fn_with_state(budget, limit))
}
async fn limit(State(budget): State<Arc<Budget>>, request: Request, next: Next) -> Response {
    if budget.check().is_err() {
        metrics::counter!("cma_browser_fault_reports_limited_total").increment(1);
        return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "1")]).into_response();
    }
    next.run(request).await
}
#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    #[tokio::test]
    async fn limits_reports_before_reading_the_body() {
        let app = router();
        let mut limited = false;
        for _ in 0..30 {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::post("/api/v1/telemetry/browser")
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(r#"{"kind":"chunk_error"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            limited |= response.status() == StatusCode::TOO_MANY_REQUESTS;
        }
        assert!(limited);
    }
}
