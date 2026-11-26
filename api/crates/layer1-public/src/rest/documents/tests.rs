use super::*;
use crate::AuthenticatedSubject;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use layer2_core::{
    document_artifacts::memory::{MemoryDocumentBlobs, MemoryDocuments},
    Authorization, Capability, SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt as _;

async fn request(
    router: &Router,
    subject: &str,
    method: &str,
    path: &str,
    key: &str,
    body: String,
) -> (StatusCode, String, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("idempotency-key", key)
        .extension(AuthenticatedSubject(SubjectId::new(subject)))
        .extension(Authorization::new([Capability::UseOwnSandboxes]))
        .body(Body::from(body))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let media_type = response.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .to_owned();
    let data = to_bytes(response.into_body(), MAX_DOCUMENT_BYTES * 2)
        .await
        .unwrap();
    (status, media_type, serde_json::from_slice(&data).unwrap())
}

#[tokio::test]
async fn document_update_honors_source_and_json_byte_boundaries() {
    let router = documents_router(DocumentService::new(
        Arc::new(MemoryDocuments::default()),
        Arc::new(MemoryDocumentBlobs::default()),
    ));
    let (status, _, created) = request(
        &router,
        "owner",
        "POST",
        "/api/v1/documents",
        "create",
        json!({"title": "Boundary fixture", "markdown": "# Small control"}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let path = format!("/api/v1/documents/{}", created["id"].as_str().unwrap());
    let source_path = format!("{path}/source");

    let ascii = "a".repeat(MAX_DOCUMENT_BYTES);
    let (status, _, updated) = request(
        &router,
        "owner",
        "PATCH",
        &path,
        "ascii",
        json!({"expected_revision": 1, "markdown": ascii}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["revision"], 2);
    assert_eq!(updated["size_bytes"], MAX_DOCUMENT_BYTES);
    let (status, _, source) =
        request(&router, "owner", "GET", &source_path, "read", String::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(source["markdown"], ascii);

    // A valid JSON encoding can use six bytes for every ASCII source byte.
    let escaped = r"\u0062".repeat(MAX_DOCUMENT_BYTES);
    let encoded = format!(r#"{{"expected_revision":2,"markdown":"{escaped}"}}"#);
    let (status, _, accepted) =
        request(&router, "owner", "PATCH", &path, "escaped", encoded.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, replayed) = request(&router, "owner", "PATCH", &path, "escaped", encoded).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed, accepted);
    assert_eq!(accepted["revision"], 3);
    assert_eq!(accepted["size_bytes"], MAX_DOCUMENT_BYTES);

    let oversized = "c".repeat(MAX_DOCUMENT_BYTES + 1);
    let (status, media, problem) = request(
        &router,
        "owner",
        "PATCH",
        &path,
        "too-big",
        json!({"expected_revision": 3, "markdown": oversized}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(media, "application/problem+json");
    assert_eq!(problem["code"], "invalid_document_request");

    let (status, media, problem) = request(
        &router,
        "owner",
        "PATCH",
        &path,
        "too-much-json",
        " ".repeat(MAX_DOCUMENT_BYTES * 6 + 16 * 1024 + 1),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(media, "application/problem+json");
    assert_eq!(problem["code"], "invalid_json");

    let (status, _, problem) = request(
        &router,
        "owner",
        "PATCH",
        &path,
        "stale",
        json!({"expected_revision": 2, "markdown": "Stale change"}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(problem["code"], "document_revision_conflict");

    let (status, _, source) =
        request(&router, "owner", "GET", &source_path, "read", String::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(source["revision"], 3);
    let markdown = source["markdown"].as_str().unwrap();
    assert_eq!(markdown, "b".repeat(MAX_DOCUMENT_BYTES));
    assert_eq!(
        accepted["sha256"],
        layer2_core::document_artifacts::digest(markdown.as_bytes())
    );
    let (status, _, _) = request(
        &router,
        "other-owner",
        "GET",
        &source_path,
        "read",
        String::new(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
