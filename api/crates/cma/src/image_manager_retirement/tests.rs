use super::*;
use axum::{body, body::Body, http::Request};
use tower::ServiceExt;

#[tokio::test]
async fn retired_requests_return_the_rest_problem() {
    let app = mount(crate::rest_router(Router::new()));
    for path in [
        "/api/image-manager",
        "/api/image-manager/",
        "/api/image-manager/v1/images",
        "/image-manager/openapi.json",
    ] {
        for method in ["GET", "HEAD", "POST", "OPTIONS"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/problem+json"
            );
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let bytes = body::to_bytes(response.into_body(), 4096).await.unwrap();
            if method == "HEAD" {
                assert!(bytes.is_empty());
            } else {
                let problem: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(problem["code"], "not_found");
                assert_eq!(problem["status"], 404);
            }
        }
    }
}

#[tokio::test]
async fn older_web_and_edge_builds_see_the_retired_capability() {
    let app = mount(crate::rest_router(Router::new()));
    let response = app
        .clone()
        .oneshot(Request::get(CAPABILITY).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let bytes = body::to_bytes(response.into_body(), 64).await.unwrap();
    assert!(serde_json::from_slice::<bool>(&bytes).unwrap());
    let response = app
        .oneshot(
            Request::get(format!("{CAPABILITY}?edge"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
