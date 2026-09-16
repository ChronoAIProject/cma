mod activity;
pub(super) mod fixture;
use axum::http::{Method, StatusCode};
use serde_json::Value;
use std::sync::atomic::Ordering;
use tower::ServiceExt;

async fn get(f: &fixture::Fixture, subject: &str, suffix: &str) -> (StatusCode, Value) {
    let path = format!("/api/v1/bridges/{}/partitions{suffix}", f.bridge.as_str());
    let mut request =
        super::bridge_invokes_tests::fixture::request(Method::GET, &path, subject, None, None);
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
async fn partition_status_is_read_only_owner_scoped_and_redacts_internal_source() {
    let f = fixture::fixture(false).await;
    let (status, page) = get(&f, "owner-a", "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(
        page["items"][0]["source"]["sender_display_name"],
        "SenderOne"
    );
    assert_eq!(page["items"][0]["source"]["chat_id"], "-100123");
    let (status, detail) = get(&f, "owner-a", &format!("/{}", f.allocation)).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["partition"], page["items"][0]);
    assert_eq!(detail["activity"]["status"], "available");
    for secret in [
        "private fixture input",
        "private-token-marker",
        "body_sha256",
        "custody",
        "key_id",
        "grant_id",
        "runtime_generation",
        "conversation_id",
    ] {
        assert!(!detail.to_string().contains(secret));
    }
    let calls = f.memory.calls.load(Ordering::SeqCst);
    assert_eq!(get(&f, "outsider", "").await.0, StatusCode::NOT_FOUND);
    assert_eq!(calls, f.memory.calls.load(Ordering::SeqCst));
    assert_eq!(
        get(&f, "owner-a", "?limit=26").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&f, "owner-a", "?cursor=invalid").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&f, "owner-a", "?owner=another").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn partition_missing_source_and_reserved_targets_are_explicit() {
    let f = fixture::fixture(false).await;
    {
        let mut evidence = f.memory.evidence.lock().unwrap();
        evidence.first_source = None;
        evidence.present.clear();
    }
    let (status, detail) = get(&f, "owner-a", &format!("/{}", f.allocation)).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["partition"]["source"]["status"], "unavailable");
    assert_eq!(detail["partition"]["target"]["status"], "reserved");
    assert_eq!(detail["activity"]["status"], "unavailable");
}

#[tokio::test]
async fn partition_org_admin_must_remain_admin_through_the_response() {
    let f = fixture::fixture(true).await;
    let suffix = format!("/{}", f.allocation);
    assert_eq!(get(&f, "owner-a", &suffix).await.0, StatusCode::OK);
    f.memory.ports.role.store(1, Ordering::SeqCst);
    assert_eq!(get(&f, "owner-a", &suffix).await.0, StatusCode::FORBIDDEN);
    f.memory.ports.role.store(3, Ordering::SeqCst);
    assert_eq!(
        get(&f, "owner-a", &suffix).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    f.memory.ports.role.store(2, Ordering::SeqCst);
    f.memory.revoke.store(true, Ordering::SeqCst);
    assert_ne!(get(&f, "owner-a", &suffix).await.0, StatusCode::OK);
}

#[tokio::test]
async fn partition_corrupt_source_or_owner_never_becomes_a_label() {
    let f = fixture::fixture(false).await;
    f.memory
        .evidence
        .lock()
        .unwrap()
        .first_source
        .as_mut()
        .unwrap()
        .envelope
        .chat_id = "another-chat".into();
    let (status, body) = get(&f, "owner-a", "").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!body.to_string().contains("another-chat"));
}

#[tokio::test]
async fn partition_manager_without_exact_target_access_receives_no_labels_or_activity() {
    let f = fixture::fixture_for(false, true).await;
    let (status, body) = get(&f, "owner-a", &format!("/{}", f.allocation)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for pointer in [
        "/partition/target/status",
        "/partition/source/status",
        "/activity/status",
    ] {
        assert_eq!(body.pointer(pointer).unwrap(), "redacted");
    }
    for value in ["SenderOne", "-100123", "agt_", "another-owner"] {
        assert!(!body.to_string().contains(value));
    }
}
