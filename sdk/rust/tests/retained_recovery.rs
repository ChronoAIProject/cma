#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{IdempotencyKey, instances::RecoverRetainedRequest};
use serde_json::json;
use wire::{WireServer, json as response};

#[tokio::test]
async fn retained_recovery_preserves_consent_revision_and_key_on_retry() {
    let server = WireServer::new().await;
    let key = IdempotencyKey::new("retained-original-key").unwrap();
    let body = RecoverRetainedRequest {
        expected_revision: 7,
        confirm_same_image_recovery: true,
    };
    for _ in 0..2 {
        let mut expected = response(
            "POST",
            "/api/v1/sandboxes/sbx_retained/runtime-recovery",
            Some(json!({"expected_revision":7,"confirm_same_image_recovery":true})),
            202,
            json!({"operation_id":"original-operation","revision":8,"accepted_at":"2026-11-11T00:00:00Z"}),
            true,
        );
        expected
            .headers
            .insert("idempotency-key".into(), "retained-original-key".into());
        server.expect(expected);
        let accepted = server
            .client
            .sandboxes()
            .recover_retained("sbx_retained", &body, &key)
            .await
            .unwrap();
        assert_eq!(accepted.operation_id, "original-operation");
        assert_eq!(accepted.revision, 8);
    }
    server.assert_drained();
    assert_eq!(server.request_count(), 2);
    let _ = (&server.anonymous, &server.base_url);
}

#[tokio::test]
async fn retained_recovery_refusal_is_not_an_accepted_operation() {
    let server = WireServer::new().await;
    let body = RecoverRetainedRequest {
        expected_revision: 7,
        confirm_same_image_recovery: true,
    };
    let key = IdempotencyKey::new("retained-original-key").unwrap();
    let mut expected = response(
        "POST",
        "/api/v1/sandboxes/sbx_retained/runtime-recovery",
        Some(json!({"expected_revision":7,"confirm_same_image_recovery":true})),
        409,
        json!({"error":{"code":"retained_recovery_refused","message":"Recovery refused"}}),
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), "retained-original-key".into());
    server.expect(expected);
    assert!(
        server
            .client
            .sandboxes()
            .recover_retained("sbx_retained", &body, &key)
            .await
            .is_err()
    );
    server.assert_drained();
    assert_eq!(server.request_count(), 1);
    let _ = (&server.anonymous, &server.base_url);
}
