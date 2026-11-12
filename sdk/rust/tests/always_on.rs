#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;

use cma_sdk::{Error, IdempotencyKey, admin::SandboxListOptions, instances::AlwaysOnRequest};
use serde_json::json;
use wire::{Expected, WireServer};

#[tokio::test]
async fn administrator_inventory_sends_true_and_false_exception_filters() {
    let server = WireServer::new().await;
    for value in [true, false] {
        server.expect(Expected {
            method: "GET".into(),
            path: format!("/api/v1/admin/sandboxes?always_on={value}"),
            headers: wire::headers(true, "application/json"),
            status: 200,
            response: serde_json::to_vec(&json!({"items": [], "next": null})).unwrap(),
            ..Default::default()
        });
        let page = server
            .client
            .admin()
            .list_sandboxes(&SandboxListOptions {
                always_on: Some(value),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(page.items.is_empty());
    }
    server.assert_drained();
}

#[tokio::test]
async fn owner_enable_preserves_the_named_refusal_without_retry() {
    let server = WireServer::new().await;
    let mut headers = wire::headers(true, "application/json");
    headers.insert("content-type".into(), "application/json".into());
    headers.insert("idempotency-key".into(), "approval-request".into());
    server.expect(Expected {
        method: "POST".into(),
        path: "/api/v1/sandboxes/sbx_exception/always-on".into(),
        headers,
        body: Some(json!({"always_on": true, "expected_revision": 1})),
        status: 403,
        response: serde_json::to_vec(&json!({"code": "always_on_approval_required",
            "detail": "An administrator must approve Always on."}))
        .unwrap(),
        ..Default::default()
    });
    let error = server
        .client
        .sandboxes()
        .set_always_on(
            "sbx_exception",
            &AlwaysOnRequest {
                always_on: true,
                expected_revision: 1,
            },
            &IdempotencyKey::new("approval-request").unwrap(),
        )
        .await
        .unwrap_err();
    assert!(!error.is_retryable());
    assert!(
        matches!(error, Error::Api { status: 403, code: Some(code), .. } if code == "always_on_approval_required")
    );
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}
