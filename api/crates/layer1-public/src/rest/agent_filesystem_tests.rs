use super::*;
use axum::{
    body::{to_bytes, Body},
    extract::DefaultBodyLimit,
    http::{Method, Request},
};
use serde_json::json;
use tower::ServiceExt as _;

#[test]
fn filesystem_openapi_contains_the_editor_and_download_contracts() {
    let document = serde_json::to_value(super::super::openapi::document()).unwrap();
    for (path, method) in [
        ("/api/v1/agents/{id}/files", "post"),
        ("/api/v1/agents/{id}/files", "delete"),
        ("/api/v1/agents/{id}/files/tree", "get"),
        ("/api/v1/agents/{id}/files/content", "get"),
        ("/api/v1/agents/{id}/files/download", "get"),
    ] {
        assert!(
            document["paths"][path][method].is_object(),
            "{method} {path}"
        );
    }
    assert!(
        document["components"]["schemas"]["WriteFileRequest"]["properties"]["expected_sha256"]
            .is_object()
    );
    assert!(
        document["components"]["schemas"]["WriteFileRequest"]["properties"]["directory"]
            .is_object()
    );
}

#[tokio::test]
async fn filesystem_route_has_a_dedicated_body_limit_and_never_caches_denials() {
    let (_, access, _, _) =
        super::super::shared_agent_tests::fixture(super::super::shared_agent_tests::Reply::Member)
            .await;
    let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
    let router = agent_filesystem_router(AgentFileService::new(access, Arc::new(adapter)))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024));
    let encoded = base64::engine::general_purpose::STANDARD.encode(vec![0; 3 * 1024 * 1024]);
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/agents/agt_shared_read/files")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({"path":"photo.bin","content_base64":encoded})).unwrap(),
        ))
        .unwrap();
    let caller = super::super::resource_access_tests::context("owner-a");
    request
        .extensions_mut()
        .insert(crate::AuthenticatedSubject(caller.caller.subject.clone()));
    request
        .extensions_mut()
        .insert(caller.caller.authorization.clone());
    request.extensions_mut().insert(caller);
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert!(
        matches!(
            value["code"].as_str(),
            Some("file_runtime_not_ready" | "file_runtime_unsupported")
        ),
        "{value}"
    );
}

#[tokio::test]
async fn filesystem_write_classifies_overlong_names_as_permanent_input_errors() {
    let (_, access, _, _) =
        super::super::shared_agent_tests::fixture(super::super::shared_agent_tests::Reply::Member)
            .await;
    let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
    let router = agent_filesystem_router(AgentFileService::new(access, Arc::new(adapter)));
    for path in [format!("{}.txt", "a".repeat(256)), "界".repeat(86)] {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/agents/agt_shared_read/files")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"path":path,"content_base64":""}).to_string(),
            ))
            .unwrap();
        let caller = super::super::resource_access_tests::context("owner-a");
        request
            .extensions_mut()
            .insert(crate::AuthenticatedSubject(caller.caller.subject.clone()));
        request
            .extensions_mut()
            .insert(caller.caller.authorization.clone());
        request.extensions_mut().insert(caller);
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(value["code"], "file_name_too_long");
        assert!(value["detail"]
            .as_str()
            .unwrap()
            .contains("255 UTF-8 bytes"));
    }
}
