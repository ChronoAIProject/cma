//! Contract tests for the declaration-only A2A surface.

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, Response, StatusCode},
};
use tower::ServiceExt;

use super::*;

#[test]
fn projection_matches_the_documented_matrix() {
    assert_eq!(
        A2aInterface.operations(),
        &[
            Operation::CreateSession,
            Operation::SendInput,
            Operation::StreamEvents,
            Operation::ResolveApproval,
            Operation::Interrupt,
            Operation::GetUsage,
        ]
    );
}

async fn assert_a2a_unavailable(response: Response<Body>, expect_body: bool) {
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&"application/problem+json".parse().unwrap())
    );
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL),
        Some(&"no-store".parse().unwrap())
    );
    if !expect_body {
        return;
    }
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("the problem document is bounded"),
    )
    .expect("the problem document is JSON");
    assert_eq!(body["status"], 501);
    assert_eq!(body["code"], "interface_not_available");
    assert_eq!(body["interface"], "a2a");
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("a2a interface")),
        "the refusal must name the unavailable interface: {body}"
    );
}

#[tokio::test]
async fn agent_card_discovery_refuses_by_name() {
    let response = router()
        .oneshot(
            Request::get("/.well-known/agent-card.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_a2a_unavailable(response, true).await;
}

#[tokio::test]
async fn every_method_on_the_documented_binding_refuses_by_name() {
    for method in [
        Method::CONNECT,
        Method::DELETE,
        Method::GET,
        Method::HEAD,
        Method::OPTIONS,
        Method::PATCH,
        Method::POST,
        Method::PUT,
        Method::TRACE,
    ] {
        let expect_body = method != Method::HEAD;
        let response = router()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/api/v1/a2a")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_a2a_unavailable(response, expect_body).await;
    }
}

#[tokio::test]
async fn an_undocumented_a2a_subpath_remains_not_found() {
    let response = router()
        .oneshot(
            Request::post("/api/v1/a2a/unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
