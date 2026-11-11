use super::*;
use axum::{
    body::to_bytes,
    http::StatusCode,
    response::IntoResponse,
    routing::{any, get, post},
    Router,
};
use http_body_util::BodyExt;
use std::{
    convert::Infallible,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;
use tower::ServiceExt;

const EVENTS: &str = "/api/v2/agents/{agent_id}/events";
const URL: &str = "/api/v2/agents/example/events";

fn protected(router: Router, budget: &Arc<StreamBudget>) -> Router {
    router.layer(axum::middleware::from_fn_with_state(
        budget.clone(),
        enforce,
    ))
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

fn slots(budget: &Arc<StreamBudget>) -> usize {
    budget.slots.as_ref().unwrap().available_permits()
}

#[tokio::test]
async fn concurrent_observers_share_admission_and_refuse_before_handler_effects() {
    let budget = StreamBudget::new(3);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let router = protected(
        Router::new().route(
            EVENTS,
            get(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async { "data: visible\n\n" }
            }),
        ),
        &budget,
    );
    let responses =
        futures_util::future::join_all((0..64).map(|_| request(&router, Method::GET, URL))).await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(slots(&budget), 0);
    let mut allowed = Vec::new();
    let mut denied = 0;
    for response in responses {
        if response.status() == StatusCode::OK {
            allowed.push(response);
        } else {
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(response.headers()[header::RETRY_AFTER], "1");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert!(response.headers().contains_key(header::LINK));
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            let problem: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(problem["code"], "public_stream_capacity_exhausted");
            denied += 1;
        }
    }
    assert_eq!((allowed.len(), denied), (3, 61));
    drop(allowed);
    assert_eq!(slots(&budget), 3);
    assert_eq!(
        request(&router, Method::GET, URL).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn every_registered_stream_method_is_charged_including_head_and_aliases() {
    let cases = [
        (
            "/api/v1/agui/events",
            "/api/v1/agui/events?threadId=example",
            Method::GET,
        ),
        (
            "/api/v1/agui/launches/{id}/events",
            "/api/v1/agui/launches/example/events",
            Method::GET,
        ),
        (EVENTS, URL, Method::GET),
        (EVENTS, URL, Method::HEAD),
        (
            "/api/v2/responses/{response_id}/events",
            "/api/v2/responses/example/events",
            Method::GET,
        ),
        ("/api/v1/agui/run", "/api/v1/agui/run", Method::POST),
        ("/api/v1/responses", "/api/v1/responses", Method::POST),
        ("/api/v2/responses", "/api/v2/responses", Method::POST),
        (
            "/api/v2/agents/{agent_id}/responses",
            "/api/v2/agents/example/responses",
            Method::POST,
        ),
    ];
    for (template, url, method) in cases {
        let budget = StreamBudget::new(1);
        let held = budget
            .slots
            .as_ref()
            .unwrap()
            .clone()
            .try_acquire_owned()
            .unwrap();
        let router = protected(
            Router::new().route(
                template,
                any(|| async {
                    panic!("saturated stream must not run its handler");
                    #[allow(unreachable_code)]
                    StatusCode::OK
                }),
            ),
            &budget,
        );
        assert_eq!(
            request(&router, method, url).await.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "{url}"
        );
        drop(held);
    }
}

#[tokio::test]
async fn recovery_controls_and_other_methods_do_not_consume_stream_slots() {
    let cases = [
        (
            "/api/v1/agents/{id}/stop",
            "/api/v1/agents/example/stop",
            Method::POST,
        ),
        (
            "/api/v1/agents/{id}/approvals/{approval_id}",
            "/api/v1/agents/example/approvals/item",
            Method::POST,
        ),
        (
            "/api/v2/agents/{agent_id}/responses/{response_id}/controls",
            "/api/v2/agents/example/responses/item/controls",
            Method::POST,
        ),
        (
            "/api/v2/agents/{agent_id}/responses/{response_id}/interrupt",
            "/api/v2/agents/example/responses/item/interrupt",
            Method::POST,
        ),
        (
            "/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation",
            "/api/v2/agents/example/responses/item/queued-cancellation",
            Method::POST,
        ),
        (
            "/api/v1/documents/{id}/shares/{subject}",
            "/api/v1/documents/example/shares/item",
            Method::DELETE,
        ),
        (
            "/api/v2/agents/{agent_id}/events/head",
            "/api/v2/agents/example/events/head",
            Method::GET,
        ),
        ("/api/v1/sandboxes", "/api/v1/sandboxes", Method::POST),
        (EVENTS, URL, Method::POST),
        (EVENTS, URL, Method::OPTIONS),
        ("/api/v2/responses", "/api/v2/responses", Method::DELETE),
        ("/other/events", "/other/events", Method::GET),
        (
            "/api/v2/agents/{agent_id}/events/extra",
            "/api/v2/agents/example/events/extra",
            Method::GET,
        ),
    ];
    let budget = StreamBudget::new(1);
    let _held = budget
        .slots
        .as_ref()
        .unwrap()
        .clone()
        .try_acquire_owned()
        .unwrap();
    for (template, url, method) in cases {
        let router = protected(
            Router::new().route(template, any(|| async { StatusCode::ACCEPTED })),
            &budget,
        );
        assert_eq!(
            request(&router, method, url).await.status(),
            StatusCode::ACCEPTED,
            "{url}"
        );
    }
}

#[tokio::test]
async fn disabled_admission_leaves_requests_and_bodies_unchanged() {
    let budget = StreamBudget::new(0);
    let router = protected(
        Router::new().route(
            EVENTS,
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    "data: unchanged\n\n",
                )
            }),
        ),
        &budget,
    );
    let mut held = Vec::new();
    for _ in 0..128 {
        held.push(request(&router, Method::GET, URL).await);
    }
    for response in held {
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            "data: unchanged\n\n"
        );
    }
}

#[tokio::test]
async fn header_wait_is_charged_and_cancelled_request_recovers_capacity() {
    let budget = StreamBudget::new(1);
    let entered = Arc::new(Notify::new());
    let observed = entered.clone();
    let router = protected(
        Router::new().route(
            EVENTS,
            get(move || {
                let observed = observed.clone();
                async move {
                    observed.notify_one();
                    std::future::pending::<Response>().await
                }
            }),
        ),
        &budget,
    );
    let running = router.clone();
    let task = tokio::spawn(async move { request(&running, Method::GET, URL).await });
    entered.notified().await;
    assert_eq!(slots(&budget), 0);
    assert_eq!(
        request(&router, Method::GET, URL).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(slots(&budget), 1);
}

#[tokio::test]
async fn body_is_lazy_and_retains_capacity_through_partial_consumption_and_eof() {
    let budget = StreamBudget::new(1);
    let polled = Arc::new(AtomicUsize::new(0));
    let observed = polled.clone();
    let router = protected(
        Router::new().route(
            EVENTS,
            get(move || {
                let observed = observed.clone();
                async move {
                    Body::from_stream(stream::iter(["first", "second"]).map(move |data| {
                        observed.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, Infallible>(data)
                    }))
                    .into_response()
                }
            }),
        ),
        &budget,
    );
    let response = request(&router, Method::GET, URL).await;
    assert_eq!(polled.load(Ordering::SeqCst), 0);
    let mut body = response.into_body();
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        "first"
    );
    assert_eq!(slots(&budget), 0);
    assert_eq!(polled.load(Ordering::SeqCst), 1);
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        "second"
    );
    assert_eq!(slots(&budget), 0);
    assert!(body.frame().await.is_none());
    assert_eq!(slots(&budget), 1);
}

#[tokio::test]
async fn consumer_disconnect_drops_pending_body_and_returns_slot() {
    let budget = StreamBudget::new(1);
    let router = protected(
        Router::new().route(
            EVENTS,
            get(|| async {
                Body::from_stream(stream::pending::<Result<&'static str, Infallible>>())
                    .into_response()
            }),
        ),
        &budget,
    );
    let response = request(&router, Method::GET, URL).await;
    let task = tokio::spawn(async move { to_bytes(response.into_body(), 1024).await });
    assert_eq!(slots(&budget), 0);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(slots(&budget), 1);
}

#[tokio::test]
async fn body_error_releases_and_terminates_without_polling_unaccounted_frames() {
    let budget = StreamBudget::new(1);
    let permit = budget
        .slots
        .as_ref()
        .unwrap()
        .clone()
        .try_acquire_owned()
        .unwrap();
    let inner = Body::from_stream(stream::iter([
        Err(std::io::Error::other("injected stream failure")),
        Ok("after failure"),
    ]));
    let mut body = retain(inner, permit);
    assert!(body.frame().await.unwrap().is_err());
    assert_eq!(slots(&budget), 1);
    assert!(body.frame().await.is_none());
}

#[tokio::test]
async fn trailers_are_preserved_and_hold_the_slot_until_end() {
    let budget = StreamBudget::new(1);
    let permit = budget
        .slots
        .as_ref()
        .unwrap()
        .clone()
        .try_acquire_owned()
        .unwrap();
    let inner = Body::new(Body::from("data").with_trailers(async {
        let mut trailers = axum::http::HeaderMap::new();
        trailers.insert("x-complete", "yes".parse().unwrap());
        Some(Ok::<_, axum::Error>(trailers))
    }));
    let mut body = retain(inner, permit);
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        "data"
    );
    assert_eq!(
        body.frame()
            .await
            .unwrap()
            .unwrap()
            .into_trailers()
            .unwrap()["x-complete"],
        "yes"
    );
    assert_eq!(slots(&budget), 0);
    assert!(body.frame().await.is_none());
    assert_eq!(slots(&budget), 1);
}

#[tokio::test]
async fn cloned_internal_dispatch_and_direct_submissions_share_one_budget() {
    let budget = StreamBudget::new(1);
    let inner = protected(
        Router::new().route("/api/v2/responses", post(|| async { "response" })),
        &budget,
    );
    let delegated = inner.clone();
    let router = inner.merge(Router::new().route(
        "/delegate",
        post(move || {
            let delegated = delegated.clone();
            async move { request(&delegated, Method::POST, "/api/v2/responses").await }
        }),
    ));
    let held = request(&router, Method::POST, "/api/v2/responses").await;
    assert_eq!(
        request(&router, Method::POST, "/delegate").await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(held);
    let held = request(&router, Method::POST, "/delegate").await;
    assert_eq!(held.status(), StatusCode::OK);
    assert_eq!(
        request(&router, Method::POST, "/api/v2/responses")
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(held);
    assert_eq!(slots(&budget), 1);
}

#[tokio::test]
async fn early_handler_refusal_still_releases_on_body_completion() {
    let budget = StreamBudget::new(1);
    let router = protected(
        Router::new().route(EVENTS, get(|| async { StatusCode::FORBIDDEN })),
        &budget,
    );
    let response = request(&router, Method::GET, URL).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(slots(&budget), 0);
    assert!(to_bytes(response.into_body(), 1024)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(slots(&budget), 1);
}
