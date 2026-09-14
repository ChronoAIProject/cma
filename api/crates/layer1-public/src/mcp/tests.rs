use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use tower::ServiceExt;

use super::*;

#[test]
fn unavailable_projection_does_not_advertise_operations() {
    assert_eq!(McpInterface.interface_id(), "mcp");
    assert!(McpInterface.operations().is_empty());
}

#[tokio::test]
async fn documented_endpoint_preserves_the_named_refusal_shape() {
    let response = router()
        .oneshot(Request::post("/api/v1/mcp").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "type": "https://spec.chrono-ai.fun/cma/1.0/problems/interface-not-available",
            "title": "interface not available",
            "status": 501,
            "detail": "The mcp interface is not yet available.",
            "code": "interface_not_available",
            "interface": "mcp",
        })
    );
}

#[tokio::test]
async fn only_post_is_claimed_at_the_documented_endpoint() {
    let response = router()
        .oneshot(Request::get("/api/v1/mcp").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn an_undocumented_subpath_remains_not_found() {
    let response = router()
        .oneshot(
            Request::post("/api/v1/mcp/unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
