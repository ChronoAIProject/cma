use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use layer2_core::{document_artifacts::*, Authorization, Caller, Capability, SubjectId};
use serde_json::{json, Value};
use tower::ServiceExt;

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
fn request(id: &str, subject: &str, key: Option<&str>, body: Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/documents/{id}"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    let mut request = builder.body(Body::from(body.to_string())).unwrap();
    request
        .extensions_mut()
        .insert(crate::AuthenticatedSubject(SubjectId::new(subject)));
    request
        .extensions_mut()
        .insert(caller(subject).authorization);
    request
}
async fn response_body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap()
}

#[tokio::test]
async fn document_delete_rest_projects_success_refusal_and_retry() {
    let rows = Arc::new(memory::MemoryDocuments::default());
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    let service = DocumentService::with_deletion(rows.clone(), blobs.clone(), false, None, true);
    let document = service
        .create(
            &caller("owner"),
            "create",
            CreateDocument {
                title: "Report".into(),
                markdown: "source".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    let router = documents_router(service);
    for (subject, key, command, status, code) in [
        (
            "other",
            Some("delete"),
            json!({"expected_revision":1}),
            404,
            "document_not_found",
        ),
        (
            "owner",
            Some("stale"),
            json!({"expected_revision":2}),
            409,
            "document_revision_conflict",
        ),
        (
            "owner",
            None,
            json!({"expected_revision":1}),
            400,
            "missing_idempotency_key",
        ),
        ("owner", Some("shape"), json!({}), 422, "invalid_json"),
    ] {
        let response = router
            .clone()
            .oneshot(request(&document.id, subject, key, command))
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(response_body(response).await["code"], code);
    }
    let response = router
        .clone()
        .oneshot(request(
            &document.id,
            "owner",
            Some("delete"),
            json!({"expected_revision":1}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let first = response_body(response).await;
    assert_eq!(first["id"], document.id);
    assert_eq!(first["revision"], 2);
    assert_eq!(first["purge"], "pending");
    let retry = router
        .clone()
        .oneshot(request(
            &document.id,
            "owner",
            Some("delete"),
            json!({"expected_revision":1}),
        ))
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(response_body(retry).await, first);
    let changed = router
        .oneshot(request(
            &document.id,
            "owner",
            Some("delete"),
            json!({"expected_revision":2}),
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_body(changed).await["code"],
        "idempotency_key_reused"
    );
    let off = DocumentService::new(rows, blobs);
    let response = documents_router(off)
        .oneshot(request(
            &document.id,
            "owner",
            Some("delete"),
            json!({"expected_revision":1}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response_body(response).await["code"],
        "document_deletion_unavailable"
    );
}

#[test]
fn document_delete_is_in_full_and_focused_openapi_with_pending_usage() {
    for contract in [
        serde_json::to_value(super::super::openapi::document()).unwrap(),
        super::super::openapi::agent_manager_document(),
    ] {
        for status in [
            "200", "400", "401", "404", "409", "413", "415", "422", "503",
        ] {
            assert!(
                contract["paths"]["/api/v1/documents/{id}"]["delete"]["responses"][status]
                    .is_object(),
                "missing {status}"
            );
        }
        assert!(contract["components"]["schemas"]["DocumentDeletion"].is_object());
        assert!(
            contract["components"]["schemas"]["DocumentUsage"]["properties"]["pending_deletions"]
                .is_object()
        );
    }
}
