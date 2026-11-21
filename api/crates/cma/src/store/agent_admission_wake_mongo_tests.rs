use std::time::Duration;

use futures_util::FutureExt;

use super::*;
use crate::store::admission_wake_test_support::CommitGate;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_admission_wake_waits_for_commit_and_rejects_failed_admission() {
    let gate = Arc::new(CommitGate::default());
    let client = gate.client().await;
    let database = format!("cma_agent_wake_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let parent = sandbox("wake");
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = workspace(&parent, "wake");
    store.create_workspace(workspace.clone()).await.unwrap();
    let input = admission(&workspace, "wake");
    let admitted = store.agent_admitted.clone();
    let mut invalid = input.clone();
    invalid.workspace_id = WorkspaceId("wks_absent".into());
    assert!(store.admit_agent(invalid).await.is_err());
    assert!(admitted.notified().now_or_never().is_none());

    let writer = store.clone();
    let write_input = input.clone();
    gate.arm();
    let task = tokio::spawn(async move { writer.admit_agent(write_input).await });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    let premature = admitted.notified().now_or_never().is_some();
    let before_commit = store.get_agent(&input.agent_id).await.unwrap();
    gate.release();
    let result = task.await.unwrap().unwrap();
    assert!(
        !premature,
        "an uncommitted admission must not wake the scheduler"
    );
    assert!(before_commit.is_none());
    assert!(matches!(result, AgentAdmissionResult::Created(_)));
    assert!(admitted.notified().now_or_never().is_some());
    assert!(store.get_agent(&input.agent_id).await.unwrap().is_some());
    assert!(admitted.notified().now_or_never().is_none());

    assert!(matches!(
        store.admit_agent(input).await.unwrap(),
        AgentAdmissionResult::Existing(_)
    ));
    assert!(admitted.notified().now_or_never().is_some());
    client.database(&database).drop().await.unwrap();
}
