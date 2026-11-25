use super::*;
use axum::{body, body::Body, http::Request};
use tower::ServiceExt;

#[tokio::test]
async fn only_enabled_replicas_refuse_retired_requests_with_the_rest_problem() {
    for enabled in [false, true] {
        let app = mount(crate::rest_router(Router::new()), enabled);
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
                if !enabled {
                    assert!(response.headers().get(header::CONTENT_TYPE).is_none());
                    continue;
                }
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
}

#[tokio::test]
async fn web_and_edge_capabilities_use_the_local_replica_setting() {
    for enabled in [false, true] {
        let app = mount(crate::rest_router(Router::new()), enabled);
        let response = app
            .clone()
            .oneshot(Request::get(CAPABILITY).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = body::to_bytes(response.into_body(), 64).await.unwrap();
        assert_eq!(serde_json::from_slice::<bool>(&bytes).unwrap(), enabled);
        let response = app
            .oneshot(
                Request::get(format!("{CAPABILITY}?edge"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if enabled {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::NO_CONTENT
            }
        );
    }
}
