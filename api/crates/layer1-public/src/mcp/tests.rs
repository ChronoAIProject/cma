//! Contract tests for the declaration-only MCP surface.

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use tower::ServiceExt;

use super::*;

#[test]
fn projection_matches_the_documented_matrix() {
    assert_eq!(
        McpInterface.operations(),
        &[
            Operation::CreateSession,
            Operation::SendInput,
            Operation::ResolveApproval,
        ]
    );
}

#[tokio::test]
async fn documented_endpoint_refuses_by_name() {
    let response = router()
        .oneshot(Request::post("/api/v1/mcp").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&"application/problem+json".parse().unwrap())
    );
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL),
        Some(&"no-store".parse().unwrap())
    );
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("the problem document is bounded"),
    )
    .expect("the problem document is JSON");
    assert_eq!(body["status"], 501);
    assert_eq!(body["code"], "interface_not_available");
    assert_eq!(body["interface"], "mcp");
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("mcp interface")),
        "the refusal must name the unavailable interface: {body}"
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
async fn an_undocumented_mcp_subpath_remains_not_found() {
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
