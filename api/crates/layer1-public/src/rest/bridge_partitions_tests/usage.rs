use super::{fixture, Ordering, StatusCode, Value};
use axum::http::Method;
use serde_json::json;
use tower::ServiceExt;

async fn get(f: &fixture::Fixture, subject: &str) -> (StatusCode, Value) {
    let path = format!("/api/v1/bridges/{}/usage", f.bridge.as_str());
    let mut request = super::super::bridge_invokes_tests::fixture::request(
        Method::GET,
        &path,
        subject,
        None,
        None,
    );
    request.extensions_mut().insert(fixture::context(subject));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("request-credential".into()));
    let response = f.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn usage_retains_retired_counts_deduplicates_lineage_and_hides_identifiers() {
    let f = fixture::fixture(false).await;
    let original = f.memory.evidence.lock().unwrap().allocation.clone();
    let (status, body) = get(&f, "owner-a").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["allocations"],
        json!({"retained":1,"current":1,"ready":0,"retired":1,"sandbox_reservations":0})
    );
    assert_eq!(body["target_resources"]["status"], "available");
    for kind in ["sandboxes", "workspaces", "worktrees", "agents"] {
        assert_eq!(
            body["target_resources"][kind],
            json!({"retained":1,"active":1}),
            "{kind}"
        );
    }
    assert_eq!(
        body["capacity_reasons"],
        json!(["shared_parent_policy_applies"])
    );
    assert!(f.memory.evidence.lock().unwrap().allocation == original);
    for secret in [
        "private fixture input",
        "private-token-marker",
        "SenderOne",
        "-100123",
        "agt_",
        "sbx_",
        "bral_",
        "runtime_generation",
        "credential",
    ] {
        assert!(!body.to_string().contains(secret), "{secret}");
    }
    let calls = f.memory.calls.load(Ordering::SeqCst);
    assert_eq!(get(&f, "outsider").await.0, StatusCode::NOT_FOUND);
    assert_eq!(calls, f.memory.calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn usage_redacts_all_target_counts_when_target_authority_is_absent() {
    let f = fixture::fixture_for(false, true).await;
    let (status, body) = get(&f, "owner-a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["allocations"]["retained"], 1);
    assert_eq!(body["target_resources"], json!({"status":"redacted"}));
    assert!(body["capacity_reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("resource_access_required")));
}

#[tokio::test]
async fn usage_inventory_limit_never_returns_truncated_totals() {
    let f = fixture::fixture(false).await;
    f.memory.usage_limited.store(true, Ordering::SeqCst);
    let (status, body) = get(&f, "owner-a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["inventory_limit"], 256);
    assert!(body["allocations"].is_null());
    assert_eq!(
        body["target_resources"],
        json!({"status":"inventory_limited"})
    );
    assert!(body["capacity_reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("inventory_limit")));
}

#[tokio::test]
async fn usage_rechecks_management_and_retained_inventory() {
    let f = fixture::fixture(true).await;
    f.memory.revoke.store(true, Ordering::SeqCst);
    assert_eq!(get(&f, "owner-a").await.0, StatusCode::FORBIDDEN);
    let f = fixture::fixture(false).await;
    f.memory.usage_changed.store(true, Ordering::SeqCst);
    assert_eq!(get(&f, "owner-a").await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn usage_reserved_targets_have_zero_retained_resource_counts() {
    let f = fixture::fixture(false).await;
    f.memory.evidence.lock().unwrap().present.clear();
    let (status, body) = get(&f, "owner-a").await;
    assert_eq!(status, StatusCode::OK);
    for kind in ["sandboxes", "workspaces", "worktrees", "agents"] {
        assert_eq!(
            body["target_resources"][kind],
            json!({"retained":0,"active":0})
        );
    }
}

#[tokio::test]
async fn usage_paused_parent_retains_resources_but_has_no_active_counts() {
    use layer2_core::sandbox::{store::SandboxStore, SandboxStatus};
    let f = fixture::fixture(false).await;
    let id = f
        .memory
        .evidence
        .lock()
        .unwrap()
        .allocation
        .placement
        .target()
        .sandbox_id
        .clone();
    let current = f.resources.get_sandbox(&id).await.unwrap().unwrap();
    let mut paused = current.clone();
    paused.status = SandboxStatus::Paused;
    paused.runtime = None;
    paused.revision += 1;
    f.resources
        .save_sandbox(current.revision, paused)
        .await
        .unwrap();
    let (status, body) = get(&f, "owner-a").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for kind in ["sandboxes", "workspaces", "worktrees", "agents"] {
        assert_eq!(
            body["target_resources"][kind],
            json!({"retained":1,"active":0})
        );
    }
}
