use super::*;

fn key(state: &SubjectAuthState) -> VerificationKey {
    VerificationKey::new(state, state.tokens.as_ref().unwrap())
}

pub(super) async fn participants(auth: &AuthState, state: &SubjectAuthState, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let joined = auth
                .fixture_cookie_verifications()
                .fixture_entries()
                .await
                .get(&key(state))
                .and_then(Weak::upgrade)
                .is_some_and(|flight| Arc::strong_count(&flight) == expected + 1);
            if joined {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("every request must join the shared in-flight verification");
}

fn spawn_request(
    auth: Arc<AuthState>,
    state: SubjectAuthState,
) -> tokio::task::JoinHandle<Result<RequestAuth, StoreError>> {
    tokio::spawn(async move { request_for_cookie(&auth, &state.subject, state.clone()).await })
}

#[tokio::test]
async fn overlapping_cookie_requests_share_one_provider_check_but_the_next_request_is_fresh() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, first) = fixture.blocked_request(state.clone()).await;
    let others = (0..15)
        .map(|_| spawn_request(fixture.auth.clone(), state.clone()))
        .collect::<Vec<_>>();
    participants(&fixture.auth, &state, 16).await;
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 3);
    fixture.control.introspection.write().await.gate = None;
    gate.release.notify_one();
    for pending in std::iter::once(first).chain(others) {
        let request = pending.await.unwrap().unwrap();
        assert_eq!(request.subject, state.subject);
        assert_eq!(
            request.authorization,
            Authorization::new([Capability::UseOwnSandboxes])
        );
    }
    assert!(fixture
        .auth
        .fixture_cookie_verifications()
        .fixture_entries()
        .await
        .is_empty());
    *fixture.control.introspection.write().await = Reply::json(json!({
        "active": true, "sub": "owner", "permissions": [],
    }));
    assert_eq!(
        fixture.request(state.clone()).await.unwrap().authorization,
        Authorization::none()
    );
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 5);
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn every_shared_caller_rechecks_current_durable_authority_before_using_evidence() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, first) = fixture.blocked_request(state.clone()).await;
    let second = spawn_request(fixture.auth.clone(), state.clone());
    participants(&fixture.auth, &state, 2).await;
    let mut changed = state.clone();
    changed.revision += 1;
    changed.token_generation += 1;
    assert!(fixture
        .store
        .replace_subject(&state.incarnation, state.revision, changed.clone())
        .await
        .unwrap());
    gate.release.notify_one();
    assert_unavailable(first.await.unwrap());
    assert_unavailable(second.await.unwrap());
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 3);
    fixture.assert_unchanged(&changed).await;
}

#[tokio::test]
async fn cancelled_initializer_does_not_cancel_a_remaining_callers_verification() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, first) = fixture.blocked_request(state.clone()).await;
    let second = spawn_request(fixture.auth.clone(), state.clone());
    participants(&fixture.auth, &state, 2).await;
    first.abort();
    assert!(first.await.is_err_and(|error| error.is_cancelled()));
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 5);
    gate.release.notify_waiters();
    assert!(second.await.unwrap().is_ok());
    assert!(fixture
        .auth
        .fixture_cookie_verifications()
        .fixture_entries()
        .await
        .is_empty());
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn cancellation_of_all_callers_retains_no_authority_and_a_new_request_starts_fresh() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, first) = fixture.blocked_request(state.clone()).await;
    first.abort();
    assert!(first.await.is_err_and(|error| error.is_cancelled()));
    assert!(fixture
        .auth
        .fixture_cookie_verifications()
        .fixture_entries()
        .await
        .get(&key(&state))
        .unwrap()
        .upgrade()
        .is_none());
    fixture.control.introspection.write().await.gate = None;
    gate.release.notify_waiters();
    assert!(fixture.request(state.clone()).await.is_ok());
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 5);
    assert!(fixture
        .auth
        .fixture_cookie_verifications()
        .fixture_entries()
        .await
        .is_empty());
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn cancelled_initializer_never_restarts_a_waiters_own_total_deadline() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, first) = fixture.blocked_request(state.clone()).await;
    let auth = fixture.auth.clone();
    let observed = state.clone();
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let second = tokio::spawn(async move {
        request_with_timeout(
            &auth,
            &observed.subject,
            observed.clone(),
            Duration::from_millis(100),
        )
        .await
    });
    participants(&fixture.auth, &state, 2).await;
    tokio::time::advance(Duration::from_millis(90)).await;
    first.abort();
    assert!(first.await.is_err_and(|error| error.is_cancelled()));
    tokio::time::advance(Duration::from_millis(11)).await;
    assert_unavailable(second.await.unwrap());
    assert!(started.elapsed() < Duration::from_millis(150));
    tokio::time::resume();
    gate.release.notify_waiters();
    assert!(fixture
        .auth
        .fixture_cookie_verifications()
        .fixture_entries()
        .await
        .get(&key(&state))
        .unwrap()
        .upgrade()
        .is_none());
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn a_timed_out_waiter_does_not_remove_a_still_pending_shared_flight() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, first) = fixture.blocked_request(state.clone()).await;
    let auth = fixture.auth.clone();
    let observed = state.clone();
    tokio::time::pause();
    let waiter = tokio::spawn(async move {
        request_with_timeout(
            &auth,
            &observed.subject,
            observed.clone(),
            Duration::from_millis(100),
        )
        .await
    });
    participants(&fixture.auth, &state, 2).await;
    tokio::time::advance(Duration::from_millis(101)).await;
    assert_unavailable(waiter.await.unwrap());
    tokio::time::resume();
    let newcomer = spawn_request(fixture.auth.clone(), state.clone());
    participants(&fixture.auth, &state, 2).await;
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 3);
    gate.release.notify_one();
    assert!(first.await.unwrap().is_ok());
    assert!(newcomer.await.unwrap().is_ok());
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn verification_registry_is_bounded_and_never_reuses_a_completed_flight() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let flights = &fixture.auth.fixture_cookie_verifications();
    let original = key(&state);
    let first = flights.acquire(&original).await;
    assert!(Arc::ptr_eq(&first, &flights.acquire(&original).await));
    assert!(first
        .set(Err(ProviderVerificationError::Unavailable(
            "fixture".into()
        )
        .into()))
        .is_ok());
    let replacement = flights.acquire(&original).await;
    assert!(!Arc::ptr_eq(&first, &replacement));
    flights.release(&original, &first).await;
    assert!(Arc::ptr_eq(&replacement, &flights.acquire(&original).await));
    let mut held = vec![replacement];
    for generation in 1..=MAX_VERIFICATION_FLIGHTS as u64 {
        let mut next = original.clone();
        next.generation = state.token_generation + generation;
        held.push(flights.acquire(&next).await);
    }
    assert_eq!(
        flights.fixture_entries().await.len(),
        MAX_VERIFICATION_FLIGHTS
    );
    drop(held);
    drop(first);
    let fresh = flights.acquire(&original).await;
    assert_eq!(flights.fixture_entries().await.len(), 1);
    assert!(!fresh.initialized());
}

#[tokio::test]
async fn verification_keys_separate_subject_incarnation_generation_access_and_expiry() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let original = key(&state);
    let flights = &fixture.auth.fixture_cookie_verifications();
    let first = flights.acquire(&original).await;
    for case in 0..5 {
        let mut next = state.clone();
        match case {
            0 => next.subject = SubjectId::new("another-owner"),
            1 => next.incarnation.push_str("-replaced"),
            2 => next.token_generation += 1,
            3 => {
                next.tokens.as_mut().unwrap().access_token =
                    SecretValue::new("different-fixture-access".into())
            }
            _ => next.tokens.as_mut().unwrap().expires_at += Duration::from_secs(1),
        }
        assert!(!Arc::ptr_eq(&first, &flights.acquire(&key(&next)).await));
    }
}
