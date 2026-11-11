use super::*;
use axum::{body::Body, http::StatusCode, routing::get, Router};
use futures_util::{stream, StreamExt};
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

const HISTORY: &str = "/api/v1/agents/{id}/history";
const URL: &str = "/api/v1/agents/a/history";

fn budget(slots: usize, bytes: usize) -> Arc<ReadBudget> {
    Arc::new(ReadBudget {
        retain_frames: false,
        slots: Arc::new(Semaphore::new(slots)),
        response_bytes: bytes,
        timeout: Duration::from_secs(2),
    })
}

fn protected(router: Router, budget: Arc<ReadBudget>) -> Router {
    router.layer(axum::middleware::from_fn_with_state(budget, enforce))
}

async fn request(router: &Router, method: Method, url: &str) -> Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(url)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn burst_refuses_before_work_and_buffers_keep_their_slots() {
    let budget = ReadBudget::new(false);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let router = protected(
        Router::new()
            .route(
                HISTORY,
                get(move || {
                    let calls = observed.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        "permitted"
                    }
                }),
            )
            .route(
                "/api/v1/agents/{id}/stop",
                axum::routing::post(|| async { StatusCode::ACCEPTED }),
            )
            .route(
                "/api/v1/documents/{id}/shares/{subject}",
                axum::routing::delete(|| async { StatusCode::NO_CONTENT }),
            ),
        budget.clone(),
    );
    let mut held = Vec::new();
    for _ in 0..CONCURRENT_READS {
        let response = request(&router, Method::GET, URL).await;
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    for _ in 0..1000 {
        let response = request(&router, Method::GET, URL).await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
    }
    assert_eq!(calls.load(Ordering::SeqCst), CONCURRENT_READS);
    assert_eq!(
        request(&router, Method::POST, "/api/v1/agents/a/stop")
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        request(&router, Method::DELETE, "/api/v1/documents/d/shares/s")
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    drop(held.pop());
    let recovered = request(&router, Method::GET, URL).await;
    assert_eq!(recovered.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(recovered.into_body(), 32).await.unwrap(),
        "permitted"
    );
    assert_eq!(budget.slots.available_permits(), 1);
    let response = request(&router, Method::GET, URL).await;
    let mut body = response.into_body().into_data_stream();
    assert_eq!(body.next().await.unwrap().unwrap(), "permitted");
    assert_eq!(budget.slots.available_permits(), 0);
    drop(body);
    assert_eq!(budget.slots.available_permits(), 1);
    drop(held);
    assert_eq!(budget.slots.available_permits(), CONCURRENT_READS);
}

#[tokio::test]
async fn actual_route_matching_covers_get_and_head_with_encoded_ids() {
    for (template, url) in [
        (HISTORY, "/api/v1/agents/%61/history"),
        (
            "/api/v1/agents/{id}/events",
            "/api/v1/agents/a/events?limit=200",
        ),
        (
            "/api/v1/agents/{id}/transcript",
            "/api/v1/agents/a/transcript",
        ),
        ("/api/v1/agents/{id}/usage", "/api/v1/agents/a/usage"),
        (
            "/api/v1/agents/{id}/resources",
            "/api/v1/agents/a/resources",
        ),
        (
            "/api/v1/agents/{id}/artifacts",
            "/api/v1/agents/a/artifacts",
        ),
        (
            "/api/v1/agents/{id}/artifacts/{artifact_id}",
            "/api/v1/agents/a/artifacts/x",
        ),
        (
            "/api/v1/agents/{id}/artifacts/{artifact_id}/download",
            "/api/v1/agents/a/artifacts/x/download",
        ),
        (
            "/api/v1/agents/{id}/files/tree",
            "/api/v1/agents/a/files/tree",
        ),
        (
            "/api/v1/agents/{id}/files/content",
            "/api/v1/agents/a/files/content",
        ),
        (
            "/api/v1/agents/{id}/files/download",
            "/api/v1/agents/a/files/download",
        ),
        ("/api/v1/documents", "/api/v1/documents"),
        ("/api/v1/documents/{id}", "/api/v1/documents/d"),
        (
            "/api/v1/documents/{id}/source",
            "/api/v1/documents/d/source",
        ),
    ] {
        let budget = budget(1, 32);
        let permit = budget.slots.clone().try_acquire_owned().unwrap();
        let router = protected(
            Router::new().route(template, get(|| async { "ok" })),
            budget,
        );
        for method in [Method::GET, Method::HEAD] {
            assert_eq!(
                request(&router, method, url).await.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "{url}"
            );
        }
        drop(permit);
        assert_eq!(
            request(&router, Method::GET, url).await.status(),
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn oversize_and_broken_bodies_fail_before_any_payload_is_released() {
    let budget = budget(1, 8);
    for length in [None, Some("1"), Some("9")] {
        let router = protected(
            Router::new().route(
                HISTORY,
                get(move || async move {
                    let mut response = Response::new(Body::from_stream(stream::iter([
                        Ok::<_, Infallible>("1234"),
                        Ok("56789"),
                    ])));
                    if let Some(length) = length {
                        response
                            .headers_mut()
                            .insert(header::CONTENT_LENGTH, length.parse().unwrap());
                    }
                    response
                }),
            ),
            budget.clone(),
        );
        let response = request(&router, Method::GET, URL).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("1234"));
        assert_eq!(budget.slots.available_permits(), 1);
    }
    let broken = protected(
        Router::new().route(
            HISTORY,
            get(|| async {
                Body::from_stream(stream::iter([
                    Ok("prefix"),
                    Err(std::io::Error::other("private failure")),
                ]))
            }),
        ),
        budget.clone(),
    );
    let response = request(&broken, Method::GET, URL).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("private failure"));
    assert_eq!(budget.slots.available_permits(), 1);

    let exact = protected(
        Router::new().route(HISTORY, get(|| async { "12345678" })),
        budget.clone(),
    );
    let response = request(&exact, Method::GET, URL).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(to_bytes(response.into_body(), 8).await.unwrap(), "12345678");
    assert_eq!(budget.slots.available_permits(), 1);
}

#[tokio::test(start_paused = true)]
async fn timeout_covers_handler_and_body_and_cancellation_releases_admission() {
    let budget = budget(1, 8);
    let entered = Arc::new(tokio::sync::Notify::new());
    let observed = entered.clone();
    let router = protected(
        Router::new().route(
            HISTORY,
            get(move || {
                let entered = observed.clone();
                async move {
                    entered.notify_one();
                    std::future::pending::<&'static str>().await
                }
            }),
        ),
        budget.clone(),
    );
    let task_router = router.clone();
    let task = tokio::spawn(async move { request(&task_router, Method::GET, URL).await });
    entered.notified().await;
    assert_eq!(budget.slots.available_permits(), 0);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(budget.slots.available_permits(), 1);
    assert_eq!(
        request(&router, Method::GET, URL).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(budget.slots.available_permits(), 1);

    let streaming = protected(
        Router::new().route(
            HISTORY,
            get(|| async {
                Body::from_stream(stream::pending::<Result<&'static str, Infallible>>())
            }),
        ),
        budget.clone(),
    );
    assert_eq!(
        request(&streaming, Method::GET, URL).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(budget.slots.available_permits(), 1);
}

#[tokio::test]
async fn admission_does_not_turn_authorization_denial_into_success() {
    let budget = budget(1, 1024);
    let router = protected(
        Router::new().route(HISTORY, get(|| async { StatusCode::NOT_FOUND })),
        budget.clone(),
    );
    let response = request(&router, Method::GET, URL).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    drop(response);
    assert_eq!(budget.slots.available_permits(), 1);
}

#[tokio::test]
async fn metadata_has_a_smaller_byte_budget_than_explicit_downloads() {
    let budget = ReadBudget::new(false);
    for (template, url, expected) in [
        (
            "/api/v1/agents/{id}/artifacts",
            "/api/v1/agents/a/artifacts",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "/api/v1/agents/{id}/artifacts/{artifact_id}",
            "/api/v1/agents/a/artifacts/x",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "/api/v1/agents/{id}/artifacts/{artifact_id}/download",
            "/api/v1/agents/a/artifacts/x/download",
            StatusCode::OK,
        ),
    ] {
        let router = protected(
            Router::new().route(template, get(|| async { vec![0_u8; METADATA_BYTES + 1] })),
            budget.clone(),
        );
        let response = request(&router, Method::GET, url).await;
        assert_eq!(response.status(), expected);
        drop(response);
        assert_eq!(budget.slots.available_permits(), CONCURRENT_READS);
    }
}

#[tokio::test]
async fn cancellation_during_body_collection_releases_the_slot() {
    let budget = budget(1, 8);
    let entered = Arc::new(tokio::sync::Notify::new());
    let observed = entered.clone();
    let router = protected(
        Router::new().route(
            HISTORY,
            get(move || {
                let entered = observed.clone();
                async move {
                    Body::from_stream(stream::once(async move {
                        entered.notify_one();
                        std::future::pending::<Result<&'static str, Infallible>>().await
                    }))
                }
            }),
        ),
        budget.clone(),
    );
    let task = tokio::spawn(async move { request(&router, Method::GET, URL).await });
    entered.notified().await;
    assert_eq!(budget.slots.available_permits(), 0);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(budget.slots.available_permits(), 1);
}

#[path = "frame_lifetime_tests.rs"]
mod frame_lifetime;
