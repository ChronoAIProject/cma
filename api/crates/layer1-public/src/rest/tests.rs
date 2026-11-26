use axum::body::Body;
use tower::ServiceExt;

use super::*;

#[tokio::test]
async fn discovery_and_openapi_are_both_composed() {
    for path in ["/api/v1/conformance", "/openapi.json"] {
        let response = router()
            .oneshot(axum::http::Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[test]
fn request_identifiers_are_canonical_and_unambiguous() {
    let canonical = "123e4567-e89b-12d3-a456-426614174000";
    assert_eq!(normalize_request_id(canonical), Some(canonical));
    assert_eq!(
        normalize_request_id(" 123e4567-e89b-12d3-a456-426614174000 "),
        Some(canonical)
    );
    assert!(normalize_request_id("123E4567-E89B-12D3-A456-426614174000").is_none());

    let mut headers = HeaderMap::new();
    headers.append("x-request-id", canonical.parse().unwrap());
    headers.append("x-request-id", canonical.parse().unwrap());
    assert!(request_id_from_headers(&headers).is_none());
}
