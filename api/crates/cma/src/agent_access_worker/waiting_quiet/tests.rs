use std::sync::atomic::{AtomicUsize, Ordering};

use layer2_core::sandbox::store::{SandboxEventStore, SandboxStore};
use layer2_core::sandbox::{SandboxWorkState, SandboxWorkStep};

use super::*;
use crate::store::agent_access_requests::tests::approved_fixture;

async fn observe_waiting(
    store: &MongoSandboxStore,
    approved: &AgentAccessRecord,
    sandbox: &Sandbox,
) {
    let calls = AtomicUsize::new(0);
    let before_events = store.sandbox_events(&sandbox.id, 0, 100).await.unwrap();
    store.verify_access_request_lineage(approved).await.unwrap();
    schedule(store, approved, sandbox, async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.read_access_request(&approved.id).await.unwrap(),
        Some(approved.clone())
    );
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert_eq!(
        store.sandbox_events(&sandbox.id, 0, 100).await.unwrap(),
        before_events
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn approved_access_waits_for_owner_resume_then_updates_the_live_runtime() {
    let (client, database, store, approved) = approved_fixture().await;
    let active = store
        .get_sandbox(&approved.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let mut pausing = active.clone();
    pausing
        .begin_pause(layer2_core::now_rfc3339(), None)
        .unwrap();
    store
        .save_sandbox(active.revision, pausing.clone())
        .await
        .unwrap();
    observe_waiting(&store, &approved, &pausing).await;

    let mut paused = pausing.clone();
    paused.status = SandboxStatus::Paused;
    paused.runtime = None;
    paused.lifecycle.operation = None;
    paused.work = SandboxWorkState::new(SandboxWorkStep::None, layer2_core::now_rfc3339());
    paused.revision += 1;
    store
        .save_sandbox(pausing.revision, paused.clone())
        .await
        .unwrap();
    observe_waiting(&store, &approved, &paused).await;

    // Restart observes the same owner approval and remains paused. No grant,
    // revival receipt or callback is invented while waiting for the owner.
    let restarted = MongoSandboxStore::open(&client, &database).await.unwrap();
    observe_waiting(&restarted, &approved, &paused).await;
    assert!(approved.progress.pause_operation_id.is_none());
    assert!(approved.progress.revive_operation_id.is_none());
    assert!(approved.progress.notification_command_id.is_none());

    // Model the lifecycle worker's completed owner-initiated resume. The access
    // worker sees Active again and atomically schedules hot credential delivery.
    let mut resumed = active.clone();
    resumed.revision = paused.revision + 1;
    resumed.lifecycle.next_authority = paused.lifecycle.next_authority + 1;
    restarted
        .save_sandbox(paused.revision, resumed.clone())
        .await
        .unwrap();
    schedule(&restarted, &approved, &resumed, async { Ok(true) })
        .await
        .unwrap();
    let scheduled = restarted
        .read_access_request(&approved.id)
        .await
        .unwrap()
        .unwrap();
    let parent = restarted.get_sandbox(&resumed.id).await.unwrap().unwrap();
    assert_eq!(scheduled.phase, Phase::Updating);
    assert_eq!(scheduled.decision, approved.decision);
    assert_eq!(scheduled.requester, approved.requester);
    assert_eq!(scheduled.lease, approved.lease);
    assert_eq!(parent.status, SandboxStatus::Active);
    assert_eq!(parent.runtime, resumed.runtime);
    assert_eq!(parent.lifecycle, resumed.lifecycle);
    assert!(parent.lifecycle.operation.is_none());
    assert!(scheduled.progress.pause_operation_id.is_none());
    let hot = scheduled.progress.hot_update.as_ref().unwrap();
    assert_eq!(Some(&hot.runtime), resumed.runtime.as_ref());
    assert_eq!(
        hot.lifecycle_next_authority,
        resumed.lifecycle.next_authority
    );
    assert!(scheduled.progress.revive_operation_id.is_none());
    assert!(scheduled.progress.notification_command_id.is_none());
    client.database(&database).drop().await.unwrap();
}
