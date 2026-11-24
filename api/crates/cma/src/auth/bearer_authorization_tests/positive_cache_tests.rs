use super::*;
use crate::auth::identity_flight::MemoryIdentityStore;
use crate::auth::{identity_lifetime, identity_retry};
use std::time::SystemTime;

async fn fixture() -> Fixture {
    let mut fixture = Fixture::new().await;
    Arc::get_mut(&mut fixture.auth).unwrap().identity_store =
        Arc::new(MemoryIdentityStore::positive_cache_prototype());
    fixture
}

async fn verify(
    fixture: &Fixture,
    token: &str,
    expiry: Option<SystemTime>,
) -> Result<Arc<VerifiedSubject>, ProviderVerificationError> {
    identity_retry::verify(
        &fixture.auth,
        Some(&fixture.metadata),
        token,
        expiry,
        None,
        identity_retry::DEADLINE,
    )
    .await
}

fn calls(fixture: &Fixture) -> usize {
    fixture.control.introspection_calls.load(Ordering::SeqCst)
}

#[tokio::test]
async fn repeated_tokens_reuse_only_their_own_evidence_until_thirty_seconds() {
    let fixture = fixture().await;
    let original = verify(&fixture, "first-fixture", None).await.unwrap();
    let repeated = verify(&fixture, "first-fixture", None).await.unwrap();
    assert!(Arc::ptr_eq(&original, &repeated));
    assert_eq!(calls(&fixture), 1);
    let unrelated = verify(&fixture, "second-fixture", None).await.unwrap();
    assert!(!Arc::ptr_eq(&original, &unrelated));
    assert_eq!(calls(&fixture), 2);

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(!identity_lifetime::current(&fixture.auth, &original));
    tokio::time::resume();
    let renewed = verify(&fixture, "first-fixture", None).await.unwrap();
    assert!(!Arc::ptr_eq(&original, &renewed));
    assert_eq!(calls(&fixture), 3);
}

#[tokio::test]
async fn a_shorter_caller_lifetime_cannot_borrow_the_cached_evidence() {
    let fixture = fixture().await;
    let original = verify(&fixture, "first-fixture", None).await.unwrap();
    let deadline = SystemTime::now() + Duration::from_secs(3);
    let shorter = verify(&fixture, "first-fixture", Some(deadline))
        .await
        .unwrap();
    assert_eq!(calls(&fixture), 2);
    assert!(!Arc::ptr_eq(&original, &shorter));
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3)).await;
    assert!(!identity_lifetime::current(&fixture.auth, &shorter));
    tokio::time::resume();
}

#[tokio::test]
async fn logout_and_invalidation_reject_in_flight_cache_publication() {
    for logout in [false, true] {
        let fixture = fixture().await;
        let gate = fixture.block().await;
        let pending = fixture.verify("first-fixture");
        fixture.arrived(1).await;
        if logout {
            fixture.auth.identity_store.clear();
        } else {
            fixture
                .auth
                .identity_store
                .invalidate_token("first-fixture");
        }
        gate.release.add_permits(1);
        assert!(pending.await.unwrap().is_err());
        *fixture.control.gate.write().await = None;
        assert!(verify(&fixture, "first-fixture", None).await.is_ok());
        assert_eq!(calls(&fixture), 2);
        fixture
            .auth
            .identity_store
            .invalidate_token("first-fixture");
        assert!(verify(&fixture, "first-fixture", None).await.is_ok());
        assert_eq!(calls(&fixture), 3);
    }
}

#[tokio::test]
async fn inactive_evidence_and_provider_failures_never_populate_the_positive_cache() {
    let fixture = fixture().await;
    fixture.control.active.store(false, Ordering::SeqCst);
    for _ in 0..2 {
        let evidence = verify(&fixture, "first-fixture", None).await;
        assert!(
            evidence.is_err()
                || evidence.is_ok_and(|result| matches!(
                    &result.permissions,
                    PermissionEvidence::Inactive
                ))
        );
    }
    assert_eq!(calls(&fixture), 2);

    fixture.control.status.store(503, Ordering::SeqCst);
    assert!(verify(&fixture, "other-fixture", None).await.is_err());
    let after_failure = calls(&fixture);
    assert!(after_failure > 2);
    fixture.control.status.store(200, Ordering::SeqCst);
    fixture.control.active.store(true, Ordering::SeqCst);
    assert!(verify(&fixture, "other-fixture", None).await.is_ok());
    assert_eq!(calls(&fixture), after_failure + 1);
}
