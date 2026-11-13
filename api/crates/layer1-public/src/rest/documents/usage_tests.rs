use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use layer2_core::{document_artifacts::*, Authorization, Capability, SubjectId};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap()
}

#[test]
fn documents_usage_is_in_full_and_focused_openapi() {
    for contract in [
        serde_json::to_value(super::super::openapi::document()).unwrap(),
        super::super::openapi::agent_manager_document(),
    ] {
        let operation = &contract["paths"]["/api/v1/usage/documents"]["get"];
        for status in ["200", "401", "404", "503"] {
            assert!(
                operation["responses"][status].is_object(),
                "missing {status} response"
            );
        }
        assert!(contract["components"]["schemas"]["DocumentUsage"].is_object());
    }
}

#[tokio::test]
async fn quota_refusal_reports_dimension_limit_and_live_without_retry_after() {
    for (dimension, code) in [
        (DocumentLimit::Documents, "document_limit_reached"),
        (DocumentLimit::Bytes, "document_bytes_limit_reached"),
        (DocumentLimit::Objects, "document_objects_limit_reached"),
    ] {
        let response = DocumentProblem(DocumentError::LimitReached {
            dimension,
            limit: 100,
            live: 99,
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(!response.headers().contains_key(header::RETRY_AFTER));
        let body = body(response).await;
        assert_eq!(
            body["type"],
            "https://spec.chrono-ai.fun/cma/1.0/problems/resource-limit-reached"
        );
        assert_eq!(body["code"], code);
        assert_eq!(body["limit"], 100);
        assert_eq!(body["live"], 99);
    }
}

#[tokio::test]
async fn receipt_or_revision_capacity_keeps_document_capacity_code() {
    let rows = Arc::new(memory::MemoryDocuments::default());
    let service = DocumentService::new(
        rows.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    );
    let authorization = Authorization::new([Capability::UseOwnSandboxes]);
    let caller = layer2_core::Caller::new(SubjectId::new("owner"), authorization.clone());
    let document = service
        .create(
            &caller,
            "create",
            CreateDocument {
                title: "Report".into(),
                markdown: "source".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    service
        .update(
            &caller,
            &document.id,
            "seed-receipt",
            UpdateDocument {
                expected_revision: 1,
                title: Some("Renamed".into()),
                markdown: None,
            },
        )
        .await
        .unwrap();
    {
        let mut records = rows.0.lock().await;
        let record = records.get_mut(&document.id).unwrap();
        let receipt = record.receipts.values().next().unwrap().clone();
        for n in record.receipts.len()..MAX_RECEIPTS {
            record.receipts.insert(format!("{n:064x}"), receipt.clone());
        }
        record.metadata.revision = MAX_RECEIPTS as u64 + 1;
        record.validate().unwrap();
    }
    let mut request = Request::builder()
        .method("PATCH")
        .uri(format!("/api/v1/documents/{}", document.id))
        .header(header::CONTENT_TYPE, "application/json")
        .header("idempotency-key", "capacity")
        .body(Body::from(
            json!({"expected_revision": MAX_RECEIPTS as u64 + 1, "title": "Blocked"}).to_string(),
        ))
        .unwrap();
    request
        .extensions_mut()
        .insert(crate::AuthenticatedSubject(SubjectId::new("owner")));
    request.extensions_mut().insert(authorization);
    let response = documents_router(service).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(!response.headers().contains_key(header::RETRY_AFTER));
    let body = body(response).await;
    assert_eq!(body["code"], "document_capacity");
    assert!(body.get("limit").is_none());
    assert!(body.get("live").is_none());
}

#[tokio::test]
async fn documents_usage_requires_authentication_and_returns_private_nullable_counters() {
    let service = DocumentService::new(
        Arc::new(memory::MemoryDocuments::default()),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    );
    let router = documents_router(service);
    for authenticated in [false, true] {
        let mut request = Request::builder()
            .uri("/api/v1/usage/documents")
            .body(Body::empty())
            .unwrap();
        if authenticated {
            request
                .extensions_mut()
                .insert(crate::AuthenticatedSubject(SubjectId::new("owner")));
            request
                .extensions_mut()
                .insert(Authorization::new([Capability::UseOwnSandboxes]));
        }
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        assert_eq!(
            response.status(),
            if authenticated {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            }
        );
        let body = body(response).await;
        if authenticated {
            assert_eq!(
                body,
                json!({"enforcement":"not_enforced", "documents":null, "bytes":null, "objects":null})
            );
        } else {
            assert_eq!(body["code"], "not_authenticated");
        }
    }
}
