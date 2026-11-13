#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{
    Error, IdempotencyKey,
    documents::{DeleteDocument, DocumentDeletion, DocumentPurgeState},
};
use serde_json::json;
use wire::WireServer;

#[tokio::test]
async fn document_delete_sends_revision_and_replays_typed_purge_states() {
    let server = WireServer::new().await;
    let key = IdempotencyKey::new("delete-fixture").unwrap();
    for (purge, state) in [
        ("pending", DocumentPurgeState::Pending),
        ("complete", DocumentPurgeState::Complete),
        ("blocked", DocumentPurgeState::Blocked),
    ] {
        let mut expected = wire::json(
            "DELETE",
            "/api/v1/documents/doc_fixture",
            Some(json!({"expected_revision":3})),
            200,
            json!({"id":"doc_fixture","revision":4,"deleted_at":"2026-11-13T10:00:00Z","purge":purge}),
            true,
        );
        expected
            .headers
            .insert("idempotency-key".into(), "delete-fixture".into());
        server.expect(expected);
        let result: DocumentDeletion = server
            .client
            .documents()
            .delete(
                "doc_fixture",
                &DeleteDocument {
                    expected_revision: 3,
                },
                &key,
            )
            .await
            .unwrap();
        assert_eq!(result.revision, 4);
        assert_eq!(result.purge, state);
    }
    let _ = (&server.anonymous, &server.base_url, server.request_count());
    server.assert_drained();
}

#[tokio::test]
async fn document_delete_preserves_named_failures_and_rejects_unknown_purge() {
    let server = WireServer::new().await;
    let key = IdempotencyKey::new("delete-fixture").unwrap();
    for (status, code) in [
        (404, "document_not_found"),
        (409, "document_revision_conflict"),
        (409, "idempotency_key_reused"),
        (503, "document_deletion_unavailable"),
        (503, "document_unavailable"),
    ] {
        let mut expected = wire::json(
            "DELETE",
            "/api/v1/documents/doc_fixture",
            Some(json!({"expected_revision":3})),
            status,
            json!({"status":status,"code":code,"title":"Deletion refused"}),
            true,
        );
        expected
            .headers
            .insert("idempotency-key".into(), "delete-fixture".into());
        server.expect(expected);
        assert!(
            matches!(server.client.documents().delete("doc_fixture", &DeleteDocument { expected_revision: 3 }, &key).await,
            Err(Error::Api { status: actual, code: Some(actual_code), .. }) if actual == status && actual_code == code)
        );
    }
    let mut expected = wire::json(
        "DELETE",
        "/api/v1/documents/doc_fixture",
        Some(json!({"expected_revision":3})),
        200,
        json!({"id":"doc_fixture","revision":4,"deleted_at":"2026-11-13T10:00:00Z","purge":"unknown"}),
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), "delete-fixture".into());
    server.expect(expected);
    assert!(matches!(
        server
            .client
            .documents()
            .delete(
                "doc_fixture",
                &DeleteDocument {
                    expected_revision: 3
                },
                &key
            )
            .await,
        Err(Error::Protocol { .. })
    ));
    let _ = (&server.anonymous, &server.base_url, server.request_count());
    server.assert_drained();
}
