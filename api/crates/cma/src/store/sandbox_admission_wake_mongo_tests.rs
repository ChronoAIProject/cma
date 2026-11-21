use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;

use super::*;
use crate::store::admission_wake_test_support::CommitGate;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_admission_wake_waits_for_commit_and_rejects_failed_admission() {
    let gate = Arc::new(CommitGate::default());
    let client = gate.client().await;
    let database = format!("cma_sandbox_wake_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let admitted = store.sandbox_admitted.clone();
    assert!(store
        .admit_sandbox(admission("sbx_denied", "denied", 0))
        .await
        .is_err());
    assert!(admitted.notified().now_or_never().is_none());

    let input = admission("sbx_new", "new", 1);
    let writer = store.clone();
    let write_input = input.clone();
    gate.arm();
    let task = tokio::spawn(async move { writer.admit_sandbox(write_input).await });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    let premature = admitted.notified().now_or_never().is_some();
    let before_commit = store.get_sandbox(&input.sandbox.id).await.unwrap();
    gate.release();
    let result = task.await.unwrap().unwrap();
    assert!(
        !premature,
        "an uncommitted admission must not wake the scheduler"
    );
    assert!(before_commit.is_none());
    assert!(matches!(result, SandboxAdmissionResult::Created(_)));
    assert!(admitted.notified().now_or_never().is_some());
    assert!(store
        .get_sandbox(&input.sandbox.id)
        .await
        .unwrap()
        .is_some());
    assert!(admitted.notified().now_or_never().is_none());

    assert!(matches!(
        store.admit_sandbox(input).await.unwrap(),
        SandboxAdmissionResult::Existing(_)
    ));
    assert!(admitted.notified().now_or_never().is_some());
    client.database(&database).drop().await.unwrap();
}
