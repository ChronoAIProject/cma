use super::*;
use layer2_core::dependency_diagnostics::{capture, Dependency, Failure, Reason};
use layer2_core::diagnostic_context::DiagnosticContext;

const TOKEN: &str = "must-not-leak-access-token";

async fn request(
    f: &Fixture,
    state: &SubjectAuthState,
    cookie: bool,
) -> Result<Option<RequestAuth>, StoreError> {
    if cookie {
        request_for_browser_cookie(&f.auth, &state.subject, state.clone()).await
    } else {
        identity::request_for_bearer(&f.auth, TOKEN.into()).await
    }
}

async fn local_capacity_survives_cookie_and_bearer_public_projection(cookie: bool) {
    let f = Fixture::new().await;
    let state = f.seed().await;
    f.auth.metadata().await.unwrap();
    let permits = f.auth.verification_slots.acquire_many(64).await.unwrap();
    let before = f.control.requests.load(Ordering::SeqCst);
    let (result, failure) = capture(request(&f, &state, cookie)).await;
    assert!(matches!(result, Err(StoreError::Unavailable(_))));
    assert_eq!(f.control.requests.load(Ordering::SeqCst), before);
    assert_eq!(
        failure,
        Some(Failure::new(Dependency::Local, Reason::Capacity))
    );
    drop(permits);
    let (result, failure) = capture(request(&f, &state, cookie)).await;
    assert!(result.unwrap().is_some());
    assert_eq!(failure, None);
    assert_eq!(f.control.requests.load(Ordering::SeqCst), before + 2);
    f.assert_unchanged(&state).await;
}

#[tokio::test]
async fn cookie_local_capacity_survives_cookie_and_bearer_public_projection() {
    local_capacity_survives_cookie_and_bearer_public_projection(true).await;
}

#[tokio::test]
async fn bearer_local_capacity_survives_cookie_and_bearer_public_projection() {
    local_capacity_survives_cookie_and_bearer_public_projection(false).await;
}

async fn local_fence_capacity_survives_cookie_and_bearer_public_projection(cookie: bool) {
    let f = Fixture::new().await;
    let state = f.seed().await;
    let mut fences = Vec::new();
    for index in 0..1024 {
        let key = f
            .auth
            .identity_store
            .key(&format!("synthetic-fence-{index}"));
        fences.push(f.auth.identity_store.token_epoch(&key).unwrap());
    }
    let before = f.control.requests.load(Ordering::SeqCst);
    let (result, failure) = capture(request(&f, &state, cookie)).await;
    assert!(matches!(result, Err(StoreError::Unavailable(_))));
    assert_eq!(f.control.requests.load(Ordering::SeqCst), before);
    assert_eq!(f.auth.verification_slots.available_permits(), 64);
    assert_eq!(
        failure,
        Some(Failure::new(Dependency::Local, Reason::Capacity))
    );
    drop(fences);
    let (result, failure) = capture(request(&f, &state, cookie)).await;
    assert!(result.unwrap().is_some());
    assert_eq!(failure, None);
}

#[tokio::test]
async fn cookie_local_fence_capacity_survives_cookie_and_bearer_public_projection() {
    local_fence_capacity_survives_cookie_and_bearer_public_projection(true).await;
}

#[tokio::test]
async fn bearer_local_fence_capacity_survives_cookie_and_bearer_public_projection() {
    local_fence_capacity_survives_cookie_and_bearer_public_projection(false).await;
}

async fn caller(
    f: &Fixture,
    state: &SubjectAuthState,
    cookie: bool,
    hash: &'static str,
) -> (bool, Option<Failure>, Arc<str>) {
    DiagnosticContext {
        request_hash: hash.into(),
        ..DiagnosticContext::default()
    }
    .scope(async {
        let (result, failure) = capture(request(f, state, cookie)).await;
        (
            matches!(result, Err(StoreError::Unavailable(_))),
            failure,
            DiagnosticContext::current().request_hash,
        )
    })
    .await
}

async fn shared_epoch_failure_is_retained_in_each_callers_own_context(cookie: bool) {
    let f = Fixture::new().await;
    let state = f.seed().await;
    f.auth.metadata().await.unwrap();
    let before = f.control.requests.load(Ordering::SeqCst);
    let gate = Arc::new(Gate::default());
    f.control.introspection.write().await.gate = Some(gate.clone());
    let first = caller(&f, &state, cookie, "leader-hash");
    let second = caller(&f, &state, cookie, "waiter-hash");
    tokio::pin!(first, second);
    assert!(futures_util::poll!(first.as_mut()).is_pending());
    tokio::select! {
        _ = &mut first => panic!("leader completed before the provider barrier"),
        reached = tokio::time::timeout(Duration::from_secs(2), gate.reached.notified()) => reached.unwrap(),
    }
    assert!(futures_util::poll!(second.as_mut()).is_pending());
    f.auth.identity_store.invalidate_token(TOKEN);
    gate.release.notify_one();
    let (first, second) = tokio::join!(first, second);
    for ((refused, failure, hash), expected) in [(first, "leader-hash"), (second, "waiter-hash")] {
        assert!(refused);
        assert_eq!(hash.as_ref(), expected);
        assert_eq!(
            failure,
            Some(Failure::new(Dependency::Local, Reason::AuthorityChanged))
        );
    }
    assert_eq!(f.control.requests.load(Ordering::SeqCst), before + 2);
    f.control.introspection.write().await.gate = None;
    let (result, failure) = capture(request(&f, &state, cookie)).await;
    assert!(result.unwrap().is_some());
    assert_eq!(failure, None);
    assert_eq!(f.control.requests.load(Ordering::SeqCst), before + 4);
}

#[tokio::test]
async fn cookie_shared_epoch_failure_is_retained_in_each_callers_own_context() {
    shared_epoch_failure_is_retained_in_each_callers_own_context(true).await;
}

#[tokio::test]
async fn bearer_shared_epoch_failure_is_retained_in_each_callers_own_context() {
    shared_epoch_failure_is_retained_in_each_callers_own_context(false).await;
}

#[tokio::test]
async fn shorter_cookie_waiter_deadline_does_not_poison_the_leader() {
    let f = Fixture::new().await;
    let state = f.seed().await;
    let gate = Arc::new(Gate::default());
    f.auth.metadata().await.unwrap();
    f.control.introspection.write().await.gate = Some(gate.clone());
    let leader = caller(&f, &state, true, "leader-hash");
    tokio::pin!(leader);
    assert!(futures_util::poll!(leader.as_mut()).is_pending());
    tokio::select! {
        _ = &mut leader => panic!("leader completed before the provider barrier"),
        reached = tokio::time::timeout(Duration::from_secs(2), gate.reached.notified()) => reached.unwrap(),
    }
    let (result, failure) = capture(request_with_timeout(
        &f.auth,
        &state.subject,
        state.clone(),
        Duration::ZERO,
    ))
    .await;
    assert_unavailable(result);
    assert_eq!(
        failure,
        Some(Failure::new(
            Dependency::Local,
            Reason::VerificationDeadline
        ))
    );
    gate.release.notify_one();
    let (refused, failure, hash) = leader.await;
    assert!(!refused);
    assert_eq!(failure, None);
    assert_eq!(hash.as_ref(), "leader-hash");
    assert_eq!(f.auth.verification_slots.available_permits(), 64);
}
