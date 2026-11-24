//! Live sharing never borrows a completed success or a newer receipt's lifetime.
use super::{
    fallback_tests::{browser, call},
    *,
};
use crate::auth::persistence::test_observer::{ObservedAuthStore, ReadGate};

async fn held_reader(
    operation: &'static str,
    short_expiry: bool,
) -> (
    Fixture,
    Arc<ReadGate>,
    tokio::task::JoinHandle<(StatusCode, Value)>,
) {
    let mut fixture = Fixture::new().await;
    fixture.seed().await;
    if short_expiry {
        let expiry = (SystemTime::now() + Duration::from_secs(3))
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        *fixture.control.introspection.write().await = Reply::json(json!({
            "active":true,"sub":"owner","permissions":["cma:sandbox:use"],"exp":expiry
        }));
    }
    let observed = Arc::new(ObservedAuthStore::new(
        fixture.store.clone(),
        Duration::ZERO,
    ));
    let auth = Arc::get_mut(&mut fixture.auth).unwrap();
    // The initial read is named "join"; the held subject/login is after verification.
    auth.enable_fixture_browser_session_join();
    auth.set_fixture_store(observed.clone());
    let (app, login) = browser(&fixture).await;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    assert_eq!(fixture.auth.fixture_active_receipts(), 0);
    let gate = Arc::new(ReadGate::default());
    *observed.held_read.lock().unwrap() = Some((operation, gate.clone()));
    let before = fixture.control.requests.load(Ordering::SeqCst);
    let pending = tokio::spawn(async move { call(&app, &login, "/auth/session").await });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .expect("live reader reaches its post-verification durable boundary");
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 2);
    (fixture, gate, pending)
}

async fn state(fixture: &Fixture) -> SubjectAuthState {
    fixture
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn successful_downgrade_fences_the_exact_receipt_held_at_custody_or_login() {
    for operation in ["subject", "login"] {
        let (fixture, gate, pending) = held_reader(operation, false).await;
        *fixture.control.introspection.write().await =
            Reply::json(json!({"active":true,"sub":"owner","permissions":[]}));
        let replacement = fixture.request(state(&fixture).await).await.unwrap();
        assert_eq!(replacement.authorization, Authorization::none());
        gate.release.notify_one();
        assert_eq!(pending.await.unwrap().0, StatusCode::SERVICE_UNAVAILABLE);
        let (app, login) = browser(&fixture).await;
        let (status, body) = call(&app, &login, "/auth/session").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["capabilities"], json!([]));
    }
}

#[tokio::test]
async fn original_provider_expiry_cannot_be_renewed_by_a_later_live_success() {
    for operation in ["subject", "login"] {
        for replace in [false, true] {
            let (fixture, gate, pending) = held_reader(operation, true).await;
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(4)).await;
            tokio::time::resume();
            if replace {
                *fixture.control.introspection.write().await = Reply::json(
                    json!({"active":true,"sub":"owner","permissions":["cma:sandbox:use"]}),
                );
                assert!(fixture
                    .request(state(&fixture).await)
                    .await
                    .unwrap()
                    .authorization
                    .allows(Capability::UseOwnSandboxes));
            }
            gate.release.notify_one();
            assert_eq!(pending.await.unwrap().0, StatusCode::SERVICE_UNAVAILABLE);
        }
    }
}

#[tokio::test]
async fn unchanged_live_receipt_survives_both_durable_read_boundaries() {
    for operation in ["subject", "login"] {
        let (fixture, gate, pending) = held_reader(operation, false).await;
        gate.release.notify_one();
        assert_eq!(pending.await.unwrap().0, StatusCode::OK);
        assert_eq!(fixture.auth.fixture_active_receipts(), 0);
    }
}

#[tokio::test]
async fn observed_provider_refusal_fences_a_held_live_receipt() {
    for operation in ["subject", "login"] {
        let (fixture, gate, pending) = held_reader(operation, false).await;
        *fixture.control.introspection.write().await = Reply::json(json!({"active":false}));
        assert_unavailable(fixture.request(state(&fixture).await).await);
        gate.release.notify_one();
        assert_eq!(pending.await.unwrap().0, StatusCode::SERVICE_UNAVAILABLE);
    }
}

#[tokio::test]
async fn equivalent_live_success_preserves_receipt_at_each_durable_boundary() {
    for operation in ["subject", "login"] {
        let (fixture, gate, pending) = held_reader(operation, false).await;
        let before = fixture.control.requests.load(Ordering::SeqCst);
        let replacement = fixture.request(state(&fixture).await).await.unwrap();
        assert!(replacement
            .authorization
            .allows(Capability::UseOwnSandboxes));
        assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 2);
        gate.release.notify_one();
        assert_eq!(pending.await.unwrap().0, StatusCode::OK);
        assert_eq!(fixture.auth.fixture_active_receipts(), 0);
    }
}

#[tokio::test]
async fn malformed_or_changed_subject_evidence_fences_each_held_boundary() {
    for operation in ["subject", "login"] {
        for reply in [
            json!({"active":true,"sub":"owner","permissions":[false]}),
            json!({"active":true,"sub":"another-owner","permissions":["cma:sandbox:use"]}),
        ] {
            let (fixture, gate, pending) = held_reader(operation, false).await;
            *fixture.control.introspection.write().await = Reply::json(reply);
            assert_unavailable(fixture.request(state(&fixture).await).await);
            gate.release.notify_one();
            assert_eq!(pending.await.unwrap().0, StatusCode::SERVICE_UNAVAILABLE);
        }
    }
}

#[tokio::test]
async fn profile_only_success_does_not_rewrite_a_held_requests_authority() {
    for operation in ["subject", "login"] {
        let (fixture, gate, pending) = held_reader(operation, false).await;
        *fixture.control.userinfo.write().await = Reply::json(json!({
            "sub":"owner","name":"New display","permissions":["cma:*"]
        }));
        let replacement = fixture.request(state(&fixture).await).await.unwrap();
        assert_eq!(
            replacement.profile.display_name.as_deref(),
            Some("New display")
        );
        assert!(replacement
            .authorization
            .allows(Capability::UseOwnSandboxes));
        gate.release.notify_one();
        let (status, _) = pending.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fixture.auth.fixture_active_receipts(), 0);
    }
}
