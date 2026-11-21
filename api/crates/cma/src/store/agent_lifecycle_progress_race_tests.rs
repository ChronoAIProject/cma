use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn lease_expiry_between_progress_read_and_write_rejects_entire_pair() {
    let fixture = Fixture::new().await;
    let writer = fixture.store.clone();
    let lease = fixture.lease.clone();
    fixture.probe.arm();
    let task = tokio::spawn(async move {
        writer
            .record_progress_many(&lease, &OBSERVATIONS[..2])
            .await
    });
    fixture.probe.wait().await;
    fixture
        .set(doc! {"lease_expires_at": DateTime::from_millis(0)})
        .await;
    fixture.probe.release();
    assert!(matches!(
        task.await.unwrap(),
        Err(StoreError::RevisionConflict { .. })
    ));
    assert!(fixture.read().await.events.is_empty());
    let next = fixture
        .store
        .claim(&fixture.lease.id)
        .await
        .unwrap()
        .unwrap()
        .lease()
        .unwrap();
    assert_ne!(next.token, fixture.lease.token);
    assert!(fixture
        .store
        .record_progress_many(&fixture.lease, &OBSERVATIONS[..2])
        .await
        .is_err());
    fixture
        .store
        .record_progress_many(&next, &OBSERVATIONS[..2])
        .await
        .unwrap();
    assert_eq!(fixture.read().await.events.len(), 2);
    fixture.db.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn progress_cas_conflict_preserves_new_second_event_for_durable_retry() {
    let fixture = Fixture::new().await;
    let writer = fixture.store.clone();
    let lease = fixture.lease.clone();
    fixture.probe.arm();
    let task = tokio::spawn(async move {
        writer
            .record_progress_many(&lease, &OBSERVATIONS[1..3])
            .await
    });
    fixture.probe.wait().await;
    let other = MongoAgentLifecycleStore::open(&fixture.db).await.unwrap();
    other
        .record_progress(
            &fixture.lease,
            "sandbox_ready",
            "ready",
            "Concurrent observation",
        )
        .await
        .unwrap();
    fixture.probe.release();
    assert!(matches!(
        task.await.unwrap(),
        Err(StoreError::RevisionConflict { .. })
    ));
    assert_eq!(fixture.read().await.events.len(), 1);
    fixture
        .store
        .record_progress_many(&fixture.lease, &OBSERVATIONS[1..3])
        .await
        .unwrap();
    let recovered = fixture.read().await;
    assert_eq!(recovered.events.len(), 2);
    assert_eq!(recovered.events[0].message, "Concurrent observation");
    assert_eq!(recovered.events[1].phase, "key_scope_ready");
    assert_eq!(recovered.events[1].sequence, 2);
    fixture
        .store
        .record_progress_many(&fixture.lease, &OBSERVATIONS[1..3])
        .await
        .unwrap();
    assert_eq!(fixture.read().await.events.len(), 2);
    fixture.db.drop().await.unwrap();
}
