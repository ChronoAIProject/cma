//! Local response-memory fences coexist with one deployment-wide caller budget.
use super::fixtures::config;
use crate::{
    expensive_reads::{self, ReadBudget},
    rate_limit::{self, RateLimiters},
    store::request_limits::MongoRequestLimits,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    middleware,
    response::Response,
    routing::get,
    Router,
};
use layer1_public::AuthenticatedSubject;
use layer2_core::SubjectId;
use mongodb::Client;
use std::{num::NonZeroU32, sync::Arc, time::Duration};
use tower::ServiceExt;

const ROUTES: [(&str, &str, &str); 3] = [
    (
        "/api/v1/agents/{id}/history",
        "/api/v1/agents/a/history",
        "1",
    ),
    ("/api/v1/agents/{id}/usage", "/api/v1/agents/a/usage", "1"),
    (
        "/api/v1/usage/metrics/view",
        "/api/v1/usage/metrics/view",
        "2",
    ),
];

async fn request(router: &Router, path: &str, subject: &str) -> Response {
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    // The real authentication middleware establishes this exact extension.
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId::new(subject)));
    tokio::time::timeout(Duration::from_secs(5), router.clone().oneshot(request))
        .await
        .expect("read admission must not queue")
        .unwrap()
}

async fn capacity(response: Response, retry: &str) {
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()[header::RETRY_AFTER], retry);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(code(response).await, "buffered_read_capacity_exhausted");
}

async fn code(response: Response) -> serde_json::Value {
    let body = to_bytes(response.into_body(), 2048).await.unwrap();
    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"].clone()
}

async fn prove(retain_frames: bool) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let name = format!("cma_read_replicas_{}", uuid::Uuid::new_v4().simple());
    // Separate Clients, limiters and ReadBudgets model two independent apps.
    let mut routers = Vec::new();
    let mut databases = Vec::new();
    for _ in 0..2 {
        let database = Client::with_uri_str(&uri).await.unwrap().database(&name);
        let store = Arc::new(MongoRequestLimits::open(&database).await.unwrap());
        let mut limits = config(true);
        limits.default_per_minute = NonZeroU32::new(4).unwrap();
        let limiter =
            RateLimiters::from_config(limits, crate::config::TrustedProxyConfig::sample(), store);
        let mut router = Router::new();
        for (template, _, _) in ROUTES {
            router = router.route(template, get(|| async { "bounded fixture response" }));
        }
        routers.push(
            router
                .layer(middleware::from_fn_with_state(limiter, rate_limit::enforce))
                .layer(middleware::from_fn_with_state(
                    ReadBudget::new(retain_frames),
                    expensive_reads::enforce,
                )),
        );
        databases.push(database);
    }
    let mut held_left = Vec::new();
    let mut held_right = Vec::new();
    for (index, (_, path, retry)) in ROUTES.iter().enumerate() {
        let subject = format!("read-lane-{index}");
        let started = tokio::time::Instant::now();
        let first = request(&routers[0], path, &subject).await;
        let second = request(&routers[0], path, &subject).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        // A saturated process does not consume B's memory reservation.
        let other = request(&routers[1], path, &subject).await;
        assert_eq!(other.status(), StatusCode::OK);
        capacity(request(&routers[0], path, &subject).await, retry).await;
        let bytes = to_bytes(first.into_body(), 1024).await.unwrap();
        if retain_frames {
            capacity(request(&routers[0], path, &subject).await, retry).await;
        }
        drop(bytes);
        // Local refusals above must not spend the fourth shared credit.
        let replacement = request(&routers[0], path, &subject).await;
        assert_eq!(replacement.status(), StatusCode::OK);
        let shared_refusal = request(&routers[1], path, &subject).await;
        assert_eq!(shared_refusal.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(code(shared_refusal).await, "rate_limited");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "fixture crossed a shared refill"
        );
        held_left.extend([second, replacement]);
        held_right.push(other);
    }
    // All three lanes on A were full together; releasing them cannot release B.
    drop(held_left);
    for (index, (_, path, retry)) in ROUTES.iter().enumerate() {
        let subject = format!("fresh-lane-{index}");
        let other = request(&routers[1], path, &subject).await;
        assert_eq!(other.status(), StatusCode::OK);
        held_right.push(other);
        capacity(request(&routers[1], path, &subject).await, retry).await;
        assert_eq!(
            request(&routers[0], path, &subject).await.status(),
            StatusCode::OK
        );
    }
    drop(held_right);
    databases[0].drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn two_read_routers_preserve_local_lanes_and_one_shared_caller_budget() {
    prove(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn retained_frames_keep_replica_local_capacity_without_minting_shared_credit() {
    prove(true).await;
}
