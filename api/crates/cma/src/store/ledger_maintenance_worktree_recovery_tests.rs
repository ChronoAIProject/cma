//! Real successor-allocation ports; no provider or filesystem effects are run.
use super::*;
use crate::store::ledger_maintenance_publication::test_support as maintenance;
use crate::store::ledger_maintenance_test_support as support;

#[path = "ledger_maintenance_worktree_recovery_race_tests.rs"]
mod races;

async fn retained_fixture() -> (Client, String, MongoSandboxStore, Sandbox, Agent) {
    let (client, name, store, mut parent, _, agent) = failed_fixture("ledger_guard").await;
    parent.runtime = crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
        "ledger-worktree",
    )
    .runtime;
    parent.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
    parent.lifecycle.runtime_generation_high_water = 1;
    support::retain_home(&mut parent);
    parent.revision += 1;
    // Fixture installation supplies the retained native binding. The operation
    // under test below is always the real authorized recovery store port.
    store
        .sandboxes
        .update_one(
            doc! {"_id":parent.id.as_str()},
            doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()},
        )
        .await
        .unwrap();
    (client, name, store, parent, agent)
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and worktree recovery hooks"]
async fn fresh_recovery_succeeds_unheld_and_both_holds_refuse_after_reopen() {
    for hold in [0, 1, 2] {
        let (client, name, store, parent, original) = retained_fixture().await;
        let fence = maintenance::fence_value(&store, &parent).await;
        if hold != 0 {
            maintenance::hold(&store, &parent, hold == 1).await;
        }
        let before = support::all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let result = recover(&reopened, &original, "recover").await;
        if hold == 0 {
            let next = result.unwrap();
            assert_eq!(next.id, original.id);
            assert_eq!(next.status, AgentStatus::Starting);
            assert_ne!(next.worktree_id, original.worktree_id);
            assert_eq!(next.conversation, original.conversation);
            assert_eq!(
                next.work.worktree_recoveries[0].original_failure,
                original.failure.unwrap()
            );
            assert_eq!(store.worktrees.count_documents(doc! {}).await.unwrap(), 2);
            assert_eq!(
                store.get_sandbox(&parent.id).await.unwrap().unwrap(),
                parent
            );
            assert_eq!(maintenance::fence_value(&store, &parent).await, fence + 1);
            assert_ne!(support::all_rows(&store).await, before);
        } else {
            assert_eq!(result, Err(AgentControlStoreError::Conflict));
            assert_eq!(support::all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and worktree recovery hooks"]
async fn recorded_recovery_remains_an_exact_zero_write_replay_while_held() {
    for v1 in [false, true] {
        let (client, name, store, parent, original) = retained_fixture().await;
        let recovered = recover(&store, &original, "recover").await.unwrap();
        maintenance::hold(&store, &parent, v1).await;
        let before = support::all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(
            recover(&reopened, &original, "recover").await.unwrap(),
            recovered
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and worktree recovery hooks"]
async fn missing_hold_audit_refuses_before_allocating_a_successor() {
    for v1 in [false, true] {
        let (client, name, store, parent, original) = retained_fixture().await;
        maintenance::hold(&store, &parent, v1).await;
        let audit = if v1 {
            crate::store::ledger_maintenance_transaction::audit_collection(&store)
        } else {
            crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
        };
        audit.delete_many(doc! {}).await.unwrap();
        let before = support::all_rows(&store).await;
        assert!(matches!(
            recover(&store, &original, "recover").await,
            Err(AgentControlStoreError::Corrupt(_))
        ));
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set with failpoints and worktree recovery hooks"]
async fn failed_commit_preserves_all_rows_then_exact_request_allocates_once() {
    let (client, name, store, parent, original) = retained_fixture().await;
    let successor = WorktreeId::generate();
    let before = support::all_rows(&store).await;
    let fence = maintenance::fence_value(&store, &parent).await;
    client.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["commitTransaction"],"errorCode":13}}).await.unwrap();
    let result = store
        .begin_agent_worktree_recovery(&original.id, &owner(), "recover", &successor, NOW.into())
        .await;
    client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(
        result,
        Err(AgentControlStoreError::Unavailable(_))
    ));
    assert_eq!(support::all_rows(&store).await, before);
    support::observe_transaction_cleanup(&store).await;
    assert_eq!(support::all_rows(&store).await, before);
    let next = store
        .begin_agent_worktree_recovery(&original.id, &owner(), "recover", &successor, NOW.into())
        .await
        .unwrap();
    assert_eq!(next.worktree_id, successor);
    assert_eq!(next.work.worktree_recoveries.len(), 1);
    assert_eq!(store.worktrees.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(maintenance::fence_value(&store, &parent).await, fence + 1);
    let written = support::all_rows(&store).await;
    assert_eq!(recover(&store, &original, "recover").await.unwrap(), next);
    assert_eq!(support::all_rows(&store).await, written);
    client.database(&name).drop().await.unwrap();
}
