use super::*;

fn failure() -> OwnershipFailure {
    OwnershipFailure::http(Phase::Challenge, 404)
}

fn failing_worker(fixture: &Fixture) -> tokio::task::JoinHandle<Result<(), OwnershipError>> {
    let authority = fixture.cancelled.clone();
    tokio::spawn(async move {
        authority
            .run(async {
                authority.record_failure_and_revoke(failure()).await;
                std::future::pending::<()>().await;
            })
            .await
    })
}

#[tokio::test(start_paused = true)]
async fn persistence_finishes_before_receiver_revocation_without_renewing_its_lease() {
    let fixture = Fixture::new(false).await;
    let token = &fixture.cancelled.token;
    let original = fixture.store.owner(&token.agent_id).await.unwrap();
    fixture.store.pause_failure.store(true, Ordering::SeqCst);
    let worker = failing_worker(&fixture);
    fixture.store.failure_entered.notified().await;
    assert!(fixture.cancelled.is_current());
    assert!(!worker.is_finished());
    assert!(fixture
        .store
        .inner
        .recorded_failure(&token.agent_id)
        .is_none());
    fixture.healthy_work().await;
    fixture.store.failure_resume.notify_one();
    assert_eq!(worker.await.unwrap(), Err(OwnershipError::Stale));
    assert_eq!(
        fixture.store.inner.recorded_failure(&token.agent_id),
        Some((token.clone(), failure()))
    );
    assert_eq!(
        fixture.store.owner(&token.agent_id).await.unwrap(),
        original
    );
    fixture
        .cancelled
        .record_failure(OwnershipFailure::http(Phase::Revoke, 403))
        .await;
    assert_eq!(fixture.store.failure_writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.store.inner.recorded_failure(&token.agent_id),
        Some((token.clone(), failure()))
    );
}

#[tokio::test(start_paused = true)]
async fn dropped_execution_scope_does_not_discard_the_first_diagnostic_write() {
    let fixture = Fixture::new(false).await;
    let token = &fixture.cancelled.token;
    fixture.store.pause_failure.store(true, Ordering::SeqCst);
    let worker = failing_worker(&fixture);
    fixture.store.failure_entered.notified().await;
    fixture.cancelled.revoke();
    assert_eq!(worker.await.unwrap(), Err(OwnershipError::Stale));
    fixture.store.failure_resume.notify_one();
    fixture
        .cancelled
        .record_failure(OwnershipFailure::http(Phase::Revoke, 403))
        .await;
    assert_eq!(fixture.store.failure_writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.store.inner.recorded_failure(&token.agent_id),
        Some((token.clone(), failure()))
    );
    assert!(!fixture.cancelled.is_current());
}

#[tokio::test(start_paused = true)]
async fn persistence_deadline_still_cancels_the_receiver_and_never_releases_its_owner() {
    let fixture = Fixture::new(false).await;
    let token = &fixture.cancelled.token;
    let original = fixture.store.owner(&token.agent_id).await.unwrap();
    fixture.store.pause_failure.store(true, Ordering::SeqCst);
    let worker = failing_worker(&fixture);
    fixture.store.failure_entered.notified().await;
    tokio::time::advance(Duration::from_millis(STORE_DEADLINE_MS)).await;
    assert_eq!(worker.await.unwrap(), Err(OwnershipError::Stale));
    assert!(fixture.cancelled.receiver_cancelled());
    assert_eq!(fixture.cancelled.receiver_failure(), Some(failure()));
    assert!(fixture
        .store
        .inner
        .recorded_failure(&token.agent_id)
        .is_none());
    assert_eq!(
        fixture.store.owner(&token.agent_id).await.unwrap(),
        original
    );
    fixture.healthy_work().await;
}

#[tokio::test(start_paused = true)]
async fn actual_expiry_cancels_without_waiting_for_diagnostics_and_rejects_the_late_write() {
    let fixture = Fixture::new(false).await;
    let token = &fixture.cancelled.token;
    tokio::time::advance(Duration::from_millis(DEFAULT_LEASE_MS - 500)).await;
    fixture.store.pause_failure.store(true, Ordering::SeqCst);
    let worker = failing_worker(&fixture);
    fixture.store.failure_entered.notified().await;
    tokio::time::advance(Duration::from_millis(500)).await;
    assert_eq!(worker.await.unwrap(), Err(OwnershipError::Stale));
    assert!(!fixture.cancelled.receiver_cancelled());
    fixture.store.failure_resume.notify_one();
    fixture
        .cancelled
        .record_failure(OwnershipFailure::http(Phase::Revoke, 403))
        .await;
    assert!(fixture
        .store
        .inner
        .recorded_failure(&token.agent_id)
        .is_none());
    assert!(!fixture.cancelled.is_current());
}

#[tokio::test(start_paused = true)]
async fn aborting_the_failure_callback_still_revokes_and_preserves_the_owned_write() {
    let fixture = Fixture::new(false).await;
    fixture.store.pause_failure.store(true, Ordering::SeqCst);
    let worker = failing_worker(&fixture);
    fixture.store.failure_entered.notified().await;
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert!(
        !fixture.cancelled.is_current(),
        "aborting persistence must still fence the receiver"
    );
    fixture.store.failure_resume.notify_one();
    fixture
        .cancelled
        .record_failure(OwnershipFailure::http(Phase::Revoke, 403))
        .await;
    assert_eq!(
        fixture
            .store
            .inner
            .recorded_failure(&fixture.cancelled.token.agent_id),
        Some((fixture.cancelled.token.clone(), failure()))
    );
}

#[tokio::test(start_paused = true)]
async fn diagnostic_switch_never_changes_store_failure_fencing_or_owner_custody() {
    for enabled in [false, true] {
        let fixture = Fixture::new(false).await;
        fixture.store.inner.enable_receiver_diagnostics(enabled);
        let original = fixture
            .store
            .owner(&fixture.cancelled.token.agent_id)
            .await
            .unwrap();
        fixture.store.inner.partition(true);
        assert!(fixture
            .cancelled
            .check_for_phase(Phase::Challenge)
            .await
            .is_err());
        assert!(!fixture.cancelled.is_current());
        assert!(fixture.cancelled.receiver_cancelled());
        assert_eq!(fixture.cancelled.receiver_failure().is_some(), enabled);
        assert_eq!(
            fixture.store.failure_writes.load(Ordering::SeqCst),
            usize::from(enabled)
        );
        fixture.store.inner.partition(false);
        assert_eq!(
            fixture
                .store
                .owner(&fixture.cancelled.token.agent_id)
                .await
                .unwrap(),
            original
        );
        fixture.healthy_work().await;
    }
}
