pub(super) mod fixture;
use axum::http::{Method, StatusCode};
use serde_json::json;
use tower::ServiceExt;

async fn send(
    f: &fixture::Fixture,
    key: Option<&str>,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let path = format!(
        "/api/v1/bridges/{}/allocations/{}/child-cleanup",
        f.bridge.as_str(),
        f.allocation
    );
    let mut request = super::bridge_invokes_tests::fixture::request(
        Method::POST,
        &path,
        "owner-a",
        key,
        Some(body),
    );
    request.extensions_mut().insert(fixture::context("owner-a"));
    let response = f.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 32768)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn explicit_cleanup_requires_retained_key_closed_body_and_preserves_exact_replay() {
    let f = fixture::fixture().await;
    assert_eq!(
        send(&f, None, fixture::body()).await.0,
        StatusCode::BAD_REQUEST
    );
    for field in [
        "owner",
        "actor",
        "runtime_id",
        "path",
        "cancel_pending_commands",
    ] {
        let mut body = fixture::body();
        body[field] = json!("untrusted");
        assert_eq!(
            send(&f, Some("closed-body"), body).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let first = send(&f, Some("exact-cleanup"), fixture::body()).await;
    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(first.1["progress"], "requested");
    assert_eq!(first.1["agent_capacity_released"], false);
    f.definitions
        .mutate(
            &fixture::context("owner-a"),
            Some("request-credential"),
            layer2_core::bridge::BridgeMutation {
                id: f.bridge.clone(),
                expected_revision: 1,
                idempotency_key: "disable-after-consent".into(),
                change: layer2_core::bridge::BridgeChange::Disable {
                    accepted_work: layer2_core::bridge::AcceptedWorkPolicy::DrainOriginal,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        send(&f, Some("exact-cleanup"), fixture::body()).await,
        first
    );
    let mut changed = fixture::body();
    changed["expected_allocation_revision"] = json!(5);
    assert_eq!(
        send(&f, Some("exact-cleanup"), changed).await.0,
        StatusCode::CONFLICT
    );
    for private in [
        "brsec_",
        "runtime-route",
        "system prompt",
        "/home/cma",
        "trust_digest",
        "body_sha256",
    ] {
        assert!(!first.1.to_string().contains(private));
    }
}
