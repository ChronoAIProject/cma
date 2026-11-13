use super::*;
use axum::{
    body::Body,
    http::{Method, StatusCode},
    middleware,
    routing::get,
    Router,
};
use std::num::NonZeroU32;
use tower::ServiceExt as _;

#[tokio::test]
async fn agent_and_desktop_reads_share_one_caller_budget_without_spending_create() {
    let quota = |n| Quota::per_minute(NonZeroU32::new(n).unwrap());
    let limits = Arc::new(RateLimiters::from_quotas(
        quota(6000),
        quota(120),
        quota(600),
        quota(10),
        quota(120),
        quota(120),
        TrustedProxyConfig::sample(),
    ));
    let app = Router::new()
        .route("/api/v1/agents/{id}", get(|| async { StatusCode::OK }))
        .route(
            "/api/v1/agents/{id}/desktop",
            get(|| async { StatusCode::OK }),
        )
        .route(
            "/api/v1/agents/{id}/desktop/ws",
            get(|| async { StatusCode::OK }),
        )
        .route("/api/v1/sandboxes/{id}", get(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state(limits.clone(), enforce));
    // Three tabs' conservative 120-read minute plus 60 opening reads each fits
    // even as an instantaneous burst. This tests allowance, not production rate.
    let routes = [
        "/api/v1/agents/a",
        "/api/v1/sandboxes/s",
        "/api/v1/agents/a/desktop",
        "/api/v1/agents/a/desktop/ws",
    ];
    for index in 0..540 {
        let mut request = Request::builder()
            .method(Method::GET)
            .uri(routes[index % routes.len()])
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(layer2_core::SubjectId::new(
                "fixture-owner",
            )));
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::OK
        );
    }
    let owner = RateKey::Subject("fixture-owner".into());
    assert!((0..100).any(|_| limits.check(Budget::Default, &owner).is_err()));
    assert!(limits
        .check(Budget::Create, &RateKey::Subject("fixture-owner".into()))
        .is_ok());
    assert!(limits
        .check(Budget::Default, &RateKey::Subject("another-owner".into()))
        .is_ok());
}
