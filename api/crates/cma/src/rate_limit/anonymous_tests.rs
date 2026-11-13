use super::anonymous::{is_anonymous_public_route, AnonymousPublicBudget};

use std::num::NonZeroU32;

#[test]
fn budget_is_shared_across_client_networks() {
    let budget =
        AnonymousPublicBudget::new(NonZeroU32::new(3).unwrap(), NonZeroU32::new(16).unwrap());

    for _ in 0..3 {
        let permit = budget
            .try_admit()
            .expect("anonymous request fits the pod budget");
        drop(permit);
    }
    let refusal = budget
        .try_admit()
        .expect_err("the fourth network must share the exhausted pod budget");
    assert!(refusal >= 1);
}

#[tokio::test]
async fn authenticated_launch_creation_is_outside_the_budget() {
    use axum::{
        body::Body,
        middleware,
        routing::{get, post},
        Router,
    };
    use tower::ServiceExt;

    let mut limiters = super::RateLimiters::from_quotas(
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        super::TrustedProxyConfig::sample(),
    );
    limiters.anonymous_public =
        AnonymousPublicBudget::new(NonZeroU32::new(1).unwrap(), NonZeroU32::new(1).unwrap());
    let app = Router::new()
        .route(
            "/api/v1/triggers/public/pub_one",
            get(|| async { "preview" }),
        )
        .route(
            "/api/v1/triggers/public/pub_one/launches",
            post(|| async { "launch" }),
        )
        .layer(middleware::from_fn_with_state(
            std::sync::Arc::new(limiters),
            super::admit,
        ));
    let request = |method: &str, path: &str| {
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        if path.ends_with("/launches") {
            request.extensions_mut().insert(super::AuthenticatedSubject(
                layer2_core::SubjectId::new("visitor"),
            ));
        }
        request
    };
    assert_eq!(
        app.clone()
            .oneshot(request("GET", "/api/v1/triggers/public/pub_one"))
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        app.oneshot(request("POST", "/api/v1/triggers/public/pub_one/launches",))
            .await
            .unwrap()
            .status(),
        200,
        "authenticated Launch creation must not consume the anonymous read budget"
    );
}

#[tokio::test]
async fn saturated_concurrency_refuses_immediately_and_recovers_permits() {
    let budget =
        AnonymousPublicBudget::new(NonZeroU32::new(100).unwrap(), NonZeroU32::new(2).unwrap());
    let first = budget.try_admit().expect("first request fits");
    let second = budget.try_admit().expect("second request fits");
    assert!(
        budget.try_admit().is_err(),
        "saturated requests must not wait"
    );

    drop(first);
    assert!(
        budget.try_admit().is_ok(),
        "dropping a response returns its permit"
    );
    drop(second);
}

#[tokio::test]
async fn anonymous_permit_is_held_for_the_request_and_released_on_cancellation() {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::{
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::get,
        Router,
    };
    use tokio::sync::Notify;
    use tower::ServiceExt;

    let mut limiters = super::RateLimiters::from_quotas(
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        super::TrustedProxyConfig::sample(),
    );
    limiters.anonymous_public =
        AnonymousPublicBudget::new(NonZeroU32::new(100).unwrap(), NonZeroU32::new(1).unwrap());

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let handler_started = Arc::clone(&started);
    let handler_release = Arc::clone(&release);
    let app = Router::new()
        .route(
            "/api/v1/triggers/public/pub_one",
            get(move || {
                let started = Arc::clone(&handler_started);
                let release = Arc::clone(&handler_release);
                async move {
                    started.notify_one();
                    release.notified().await;
                    StatusCode::NO_CONTENT
                }
            }),
        )
        .layer(middleware::from_fn_with_state(
            Arc::new(limiters),
            super::admit,
        ));
    let request = || {
        Request::get("/api/v1/triggers/public/pub_one")
            .body(Body::empty())
            .unwrap()
    };
    let first = tokio::spawn({
        let app = app.clone();
        async move { app.oneshot(request()).await.unwrap() }
    });
    started.notified().await;

    let second = tokio::time::timeout(Duration::from_millis(250), {
        let app = app.clone();
        async move { app.oneshot(request()).await.unwrap() }
    })
    .await
    .expect("the second request must be refused without waiting");
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);

    first.abort();
    assert!(
        first.await.is_err(),
        "the first request should be cancelled"
    );

    let third = tokio::spawn({
        let app = app.clone();
        async move { app.oneshot(request()).await.unwrap() }
    });
    started.notified().await;
    release.notify_one();
    assert_eq!(third.await.unwrap().status(), StatusCode::NO_CONTENT);

    let fourth = tokio::spawn({
        let app = app.clone();
        async move { app.oneshot(request()).await.unwrap() }
    });
    started.notified().await;
    let while_fourth_is_running = tokio::time::timeout(Duration::from_millis(250), {
        let app = app.clone();
        async move { app.oneshot(request()).await.unwrap() }
    })
    .await
    .expect("a running request must retain the only permit");
    assert_eq!(
        while_fourth_is_running.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    release.notify_one();
    assert_eq!(fourth.await.unwrap().status(), StatusCode::NO_CONTENT);

    let fifth = tokio::spawn({
        let app = app.clone();
        async move { app.oneshot(request()).await.unwrap() }
    });
    started.notified().await;
    release.notify_one();
    assert_eq!(fifth.await.unwrap().status(), StatusCode::NO_CONTENT);
}

#[test]
fn routes_match_exact_templates() {
    for path in [
        "/api/v1/triggers/public/pub_one",
        "/api/v1/triggers/public/pub_one/badge.svg",
        "/api/v1/triggers/public/pub_one/open/plc_one",
        "/api/v1/triggers/public/pub_one/entry-intent",
    ] {
        assert!(is_anonymous_public_route(path), "{path}");
    }
    for path in [
        "/api/v1/triggers/public/pub_one/launches",
        "/api/v1/triggers/pub_one",
        "/api/v1/triggers/public/pub_one/open/plc_one/extra",
    ] {
        assert!(!is_anonymous_public_route(path), "{path}");
    }
}

#[tokio::test]
async fn refusal_precedes_downstream_budget_io() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use axum::{body::Body, middleware, routing::get, Router};
    use tower::ServiceExt;

    let mut limiters = super::RateLimiters::from_quotas(
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        governor::Quota::per_minute(NonZeroU32::new(100).unwrap()),
        super::TrustedProxyConfig::sample(),
    );
    limiters.anonymous_public =
        AnonymousPublicBudget::new(NonZeroU32::new(1).unwrap(), NonZeroU32::new(1).unwrap());
    let limiters = std::sync::Arc::new(limiters);
    let downstream_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&downstream_calls);
    let app = Router::new()
        .route(
            "/api/v1/triggers/public/pub_one",
            get(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { "ok" }
            }),
        )
        .layer(middleware::from_fn_with_state(limiters, super::admit));

    let request = || {
        axum::http::Request::builder()
            .uri("/api/v1/triggers/public/pub_one")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(app.clone().oneshot(request()).await.unwrap().status(), 200);
    assert_eq!(app.clone().oneshot(request()).await.unwrap().status(), 429);
    assert_eq!(downstream_calls.load(Ordering::SeqCst), 1);
}
