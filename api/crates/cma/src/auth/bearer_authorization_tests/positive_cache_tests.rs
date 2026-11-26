use super::*;
use crate::auth::{
    identity_flight::MemoryIdentityStore, identity_lifetime, identity_retry, router,
};
use axum::{body::Body, http::Request};
use std::time::SystemTime;
use tower::ServiceExt;

const ACCESS: &str = "fixture-access";

fn enable(auth: &mut Arc<AuthState>) {
    Arc::get_mut(auth).unwrap().identity_store =
        Arc::new(MemoryIdentityStore::positive_cache_prototype());
}

async fn fixture() -> Fixture {
    let mut f = Fixture::new().await;
    enable(&mut f.auth);
    f
}

async fn check(f: &Fixture, token: &str) -> Arc<VerifiedSubject> {
    verify_subject(&f.auth, &f.metadata, token).await.unwrap()
}

fn calls(f: &Fixture) -> usize {
    let userinfo = f.control.userinfo_calls.load(Ordering::SeqCst);
    let introspection = f.control.introspection_calls.load(Ordering::SeqCst);
    assert_eq!(userinfo, introspection);
    userinfo
}

#[tokio::test]
async fn hits_keep_original_deadlines_and_admission_is_bounded() {
    let f = fixture().await;
    let first = check(&f, ACCESS).await;
    check(&f, "fixture-other").await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(Arc::ptr_eq(&first, &check(&f, ACCESS).await));
    assert_eq!(calls(&f), 2);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(!identity_lifetime::current(&f.auth, &first));
    tokio::time::resume();
    let renewed = check(&f, ACCESS).await;
    assert!(!Arc::ptr_eq(&first, &renewed));
    for index in 0..127 {
        check(&f, &format!("fixture-{index}")).await;
    }
    let uncached = check(&f, "fixture-overflow").await;
    assert!(!Arc::ptr_eq(
        &uncached,
        &check(&f, "fixture-overflow").await
    ));
    assert!(Arc::ptr_eq(&renewed, &check(&f, ACCESS).await));
    assert_eq!(calls(&f), 132);
}

#[tokio::test]
async fn cache_misses_for_other_scope_subject_or_shorter_custody() {
    let f = fixture().await;
    let first = check(&f, ACCESS).await;
    let store = &f.auth.identity_store;
    let cache = store.positive_cache().unwrap();
    let key = store.key(ACCESS);
    let mut metadata = f.metadata.clone();
    metadata.authorization_endpoint.push_str("/other");
    assert!(cache.cached_positive(&key, &metadata, None, None).is_none());
    assert!(cache
        .cached_positive(&key, &f.metadata, None, Some("other"))
        .is_none());
    let expiry = SystemTime::now() + Duration::from_secs(3);
    let short = identity_retry::verify(
        &f.auth,
        Some(&f.metadata),
        ACCESS,
        Some(expiry),
        Some("fixture-owner"),
        identity_retry::DEADLINE,
    )
    .await
    .unwrap();
    assert!(!Arc::ptr_eq(&first, &short));
    assert_eq!(calls(&f), 2);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3)).await;
    assert!(!identity_lifetime::current(&f.auth, &short));
}

#[tokio::test]
async fn logout_clears_a_hit_revokes_cookie_and_rejects_pending_publication() {
    let f = fixture().await;
    let first = check(&f, ACCESS).await;
    let gate = f.block().await;
    let pending = f.verify("fixture-pending");
    f.arrived(2).await;
    let cookie = f
        .auth
        .issue_browser_login(first.subject.clone())
        .await
        .unwrap();
    let logout = Request::get("/auth/logout")
        .header("cookie", format!("cma_session={cookie}"))
        .body(Body::empty())
        .unwrap();
    let response = router(f.auth.clone()).oneshot(logout).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let login = SecretDigest::identify(crate::auth::AUTH_COOKIE_DOMAIN, &cookie);
    assert!(f.auth.store.browser_login(&login).await.unwrap().is_none());
    gate.release.add_permits(1);
    assert!(pending.await.unwrap().is_err());
    *f.control.gate.write().await = None;
    assert!(!identity_lifetime::current(&f.auth, &first));
    assert!(!Arc::ptr_eq(&first, &check(&f, ACCESS).await));
    assert_eq!(calls(&f), 3);
}

#[tokio::test]
async fn observed_revocation_fences_late_writes_but_is_local() {
    let a = fixture().await;
    let mut b = AuthState::in_memory(a.auth.mode.clone(), None);
    enable(&mut b);
    let first = check(&a, ACCESS).await;
    let peer = verify_subject(&b, &a.metadata, ACCESS).await.unwrap();
    let store = &a.auth.identity_store;
    let key = store.key(ACCESS);
    let epoch = store.token_epoch(&key).unwrap();
    store.invalidate_token(ACCESS);
    let _replacement = store.token_epoch(&key).unwrap();
    store
        .positive_cache()
        .unwrap()
        .remember_positive(&key, &epoch, a.metadata.clone(), first);
    a.control.active.store(false, Ordering::SeqCst);
    for _ in 0..2 {
        let refused = verify_subject(&a.auth, &a.metadata, ACCESS).await;
        assert!(matches!(
            refused,
            Err(ProviderVerificationError::Rejected(_))
        ));
    }
    assert_eq!(calls(&a), 4);
    let stale = verify_subject(&b, &a.metadata, ACCESS).await.unwrap();
    assert!(Arc::ptr_eq(&peer, &stale));
    assert_eq!(calls(&a), 4);
    a.control.status.store(503, Ordering::SeqCst);
    assert!(verify_subject(&a.auth, &a.metadata, ACCESS).await.is_err());
    let failed_calls = a.control.introspection_calls.load(Ordering::SeqCst);
    a.control.status.store(200, Ordering::SeqCst);
    a.control.active.store(true, Ordering::SeqCst);
    check(&a, ACCESS).await;
    assert_eq!(calls(&a), failed_calls + 1);
}
