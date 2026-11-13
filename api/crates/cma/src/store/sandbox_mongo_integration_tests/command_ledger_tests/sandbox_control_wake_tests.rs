use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_control_wake_waits_for_applied_durable_commit() {
    use crate::store::admission_wake_test_support::CommitGate;
    use futures_util::FutureExt;
    let gate = Arc::new(CommitGate::default());
    let client = gate.client().await;
    let name = format!("cma_control_wake_{}", uuid::Uuid::new_v4().simple());
    let f = Fixture::new(&client, &name).await;
    let wake = f.store.sandbox_admitted.clone();
    let record = f.admit("wake").await.unwrap();
    assert!(
        wake.notified().now_or_never().is_none(),
        "queued ledger alone cannot wake lifecycle execution"
    );
    let proof = f.proof(&record, MAX_MEMBERSHIP_AGE).await;
    let store = f.store.clone();
    gate.arm();
    let task = tokio::spawn(async move { store.apply_sandbox_control(&record, proof).await });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    let premature = wake.notified().now_or_never().is_some();
    let before = f.store.get_sandbox(&f.parent.id).await.unwrap().unwrap();
    gate.release();
    let applied = task.await.unwrap().unwrap();
    assert!(!premature);
    assert_eq!(before.status, SandboxStatus::Active);
    assert!(matches!(
        applied.status,
        SandboxControlStatus::Applied { .. }
    ));
    assert!(wake.notified().now_or_never().is_some());
    assert!(wake.notified().now_or_never().is_none());
    assert_eq!(
        f.store
            .get_sandbox(&f.parent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Pausing
    );
    client.database(&name).drop().await.unwrap();
    client.shutdown().await;
}
