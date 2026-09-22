use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, Response, StatusCode},
};
use tower::ServiceExt;

use super::*;

#[test]
fn unavailable_projection_does_not_advertise_operations() {
    assert_eq!(A2aInterface.interface_id(), "a2a");
    assert!(A2aInterface.operations().is_empty());
}

async fn assert_named_refusal(response: Response<Body>, expect_body: bool) {
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    if !expect_body {
        assert!(bytes.is_empty());
        return;
    }
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "type": "https://spec.chrono-ai.fun/cma/1.0/problems/interface-not-available",
            "title": "interface not available",
            "status": 501,
            "detail": "The a2a interface is not yet available.",
            "code": "interface_not_available",
            "interface": "a2a",
        })
    );
}

#[tokio::test]
async fn agent_card_discovery_advertises_the_verified_provider_contract() {
    let response = router("https://cma.example")
        .oneshot(
            Request::get("/.well-known/agent-card.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "name":"CMA Codex provider", "version":env!("CARGO_PKG_VERSION"), "url":"https://cma.example",
            "capabilities":{"streaming":true,"activity_items":true,"client_tools":false,"cancel":true,"rich_input":false},
            "provider_contract":{"binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},
            "authentication":{"schemes":["bearer-agent-key"]}
        })
    );
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
        let response = router("https://cma.example")
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/api/v1/a2a")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_named_refusal(response, expect_body).await;
    }
}

#[tokio::test]
async fn an_undocumented_subpath_remains_not_found() {
    let response = router("https://cma.example")
        .oneshot(
            Request::post("/api/v1/a2a/unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
