use super::fixture;
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use tower::ServiceExt;

async fn send(f: &fixture::Fixture, key: Option<&str>, body: Value) -> (StatusCode, Value) {
    let path = format!(
        "/api/v1/bridges/{}/allocations/{}/workspace-cleanup",
        f.bridge.as_str(),
        f.allocation
    );
    let mut request = super::super::bridge_invokes_tests::fixture::request(
        Method::POST,
        &path,
        "owner-a",
        key,
        Some(body),
    );
    request.extensions_mut().insert(fixture::context("owner-a"));
    let response = f.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 32768)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn workspace_consent_is_closed_and_original_key_recovers_after_capability_and_revision_change(
) {
    let f = fixture::owned_fixture().await;
    assert_eq!(
        send(&f, None, fixture::workspace_body()).await.0,
        StatusCode::BAD_REQUEST
    );
    for field in [
        "owner",
        "actor",
        "runtime_id",
        "path",
        "cascade",
        "target",
        "replacement_key",
        "cancel_pending_commands",
    ] {
        let mut body = fixture::workspace_body();
        body[field] = json!("untrusted");
        assert_eq!(
            send(&f, Some("closed-workspace"), body).await.0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}"
        );
    }
    for field in [
        "expected_workspace_revision",
        "child_cleanup_receipt_id",
        "action",
    ] {
        let mut body = fixture::workspace_body();
        body.as_object_mut().unwrap().remove(field);
        assert_eq!(
            send(&f, Some("incomplete-workspace"), body).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let first = send(&f, Some("exact-workspace"), fixture::workspace_body()).await;
    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(first.1["workspace_capacity_released"], false);
    assert_eq!(first.1["parent_retained"], true);
    assert_eq!(first.1["eligible"], false);
    f.runtime_supported(false);
    f.definitions
        .mutate(
            &fixture::context("owner-a"),
            Some("request-credential"),
            layer2_core::bridge::BridgeMutation {
                id: f.bridge.clone(),
                expected_revision: 1,
                idempotency_key: "disable-after-workspace-consent".into(),
                change: layer2_core::bridge::BridgeChange::Disable {
                    accepted_work: layer2_core::bridge::AcceptedWorkPolicy::DrainOriginal,
                },
            },
        )
        .await
        .unwrap();
    let recovered = send(&f, Some("exact-workspace"), fixture::workspace_body()).await;
    assert_eq!(recovered.0, StatusCode::ACCEPTED);
    assert_eq!(recovered.1["receipt"], first.1["receipt"]);
    assert_eq!(recovered.1["runtime_supported"], false);
    for field in [
        "expected_bridge_revision",
        "expected_allocation_revision",
        "expected_workspace_revision",
    ] {
        let mut changed = fixture::workspace_body();
        changed[field] = json!(8);
        assert_eq!(
            send(&f, Some("exact-workspace"), changed).await.0,
            StatusCode::CONFLICT
        );
    }
    let mut changed = fixture::workspace_body();
    changed["child_cleanup_receipt_id"] = json!(format!("bclj_{}", "8".repeat(64)));
    assert_eq!(
        send(&f, Some("exact-workspace"), changed).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(&f, Some("replacement-workspace"), fixture::workspace_body())
            .await
            .0,
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn workspace_consent_refuses_ineligible_parent_and_does_not_change_child_effect_scope() {
    for condition in ["not_owned", "child_incomplete", "unsupported"] {
        let f = if condition == "not_owned" {
            fixture::fixture().await
        } else {
            fixture::owned_fixture().await
        };
        if condition == "child_incomplete" {
            f.memory.child_complete.store(false, Ordering::SeqCst);
        }
        if condition == "unsupported" {
            f.runtime_supported(false);
        }
        assert_eq!(
            send(&f, Some("reviewed-workspace"), fixture::workspace_body())
                .await
                .0,
            StatusCode::CONFLICT
        );
        let path = format!(
            "/api/v1/bridges/{}/allocations/{}/child-cleanup",
            f.bridge.as_str(),
            f.allocation
        );
        let mut request = super::super::bridge_invokes_tests::fixture::request(
            Method::GET,
            &path,
            "owner-a",
            None,
            None,
        );
        request.extensions_mut().insert(fixture::context("owner-a"));
        let response = f.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 32768)
            .await
            .unwrap();
        let child: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(child["workspace_retained"], true);
        assert_eq!(child["receipt"], Value::Null);
    }
}
impl fixture::Fixture {
    fn runtime_supported(&self, value: bool) {
        self.memory.runtime_supported.store(value, Ordering::SeqCst);
    }
}
