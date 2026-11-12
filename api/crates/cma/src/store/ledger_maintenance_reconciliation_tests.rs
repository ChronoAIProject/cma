//! Exercise the real runtime-loss transaction under durable maintenance holds.
use super::*;
use crate::store::ledger_maintenance_publication::test_support as maintenance;
use crate::store::ledger_maintenance_test_support as support;

#[path = "ledger_maintenance_reconciliation_race_tests.rs"]
mod races;

async fn retained_fixture(store: &MongoSandboxStore, checkpointed: bool) -> (Sandbox, Agent) {
    let (mut parent, agent) = if checkpointed {
        checkpoint_fixture(store, "sbx_ledger_reconciliation").await
    } else {
        fixture(store, "sbx_ledger_reconciliation").await
    };
    let revision = parent.revision;
    parent.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
    support::retain_home(&mut parent);
    parent.revision += 1;
    store.save_sandbox(revision, parent.clone()).await.unwrap();
    support::insert_hierarchy(store, &parent, &agent).await;
    (parent, agent)
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and reconciliation hooks"]
async fn runtime_loss_has_positive_controls_and_both_holds_refuse_after_reopen() {
    for checkpointed in [false, true] {
        for hold in [0, 1, 2] {
            let (client, name, store) = database().await;
            let (parent, agent) = retained_fixture(&store, checkpointed).await;
            let fence = maintenance::fence_value(&store, &parent).await;
            if hold != 0 {
                maintenance::hold(&store, &parent, hold == 1).await;
            }
            let before = support::all_rows(&store).await;
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let result = reopened.recover_runtime_loss(loss(&parent)).await;
            if hold == 0 {
                assert_eq!(
                    result.unwrap(),
                    if checkpointed {
                        SandboxRuntimeLossOutcome::RecoveryScheduled
                    } else {
                        SandboxRuntimeLossOutcome::Failed
                    }
                );
                let next = store.get_sandbox(&parent.id).await.unwrap().unwrap();
                assert_eq!(
                    next.status,
                    if checkpointed {
                        SandboxStatus::Reviving
                    } else {
                        SandboxStatus::Failed
                    }
                );
                assert!(next.revision > parent.revision);
                assert_eq!(next.lifecycle.home, parent.lifecycle.home);
                assert_eq!(maintenance::fence_value(&store, &parent).await, fence + 1);
                assert_eq!(
                    store.get_agent(&agent.id).await.unwrap().unwrap().status,
                    if checkpointed {
                        AgentStatus::Paused
                    } else {
                        AgentStatus::Stopped
                    }
                );
                let committed = support::all_rows(&store).await;
                assert_ne!(committed, before);
                assert_eq!(
                    reopened.recover_runtime_loss(loss(&parent)).await.unwrap(),
                    SandboxRuntimeLossOutcome::Superseded
                );
                assert_eq!(support::all_rows(&store).await, committed);
            } else {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_eq!(support::all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and reconciliation hooks"]
async fn runtime_loss_refuses_missing_audit_before_any_changes() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let (parent, _) = retained_fixture(&store, true).await;
        maintenance::hold(&store, &parent, v1).await;
        let audit = if v1 {
            crate::store::ledger_maintenance_transaction::audit_collection(&store)
        } else {
            crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
        };
        audit.delete_many(doc! {}).await.unwrap();
        let before = support::all_rows(&store).await;
        assert!(matches!(
            store.recover_runtime_loss(loss(&parent)).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and reconciliation hooks"]
async fn held_runtime_loss_refuses_before_corrupt_history_can_quarantine() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let (parent, agent) = retained_fixture(&store, true).await;
        maintenance::hold(&store, &parent, v1).await;
        store
            .agent_log
            .delete_many(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap();
        let before = support::all_rows(&store).await;
        assert_eq!(
            store.recover_runtime_loss(loss(&parent)).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set with failpoints and reconciliation hooks"]
async fn failed_reconciliation_commit_rolls_back_fence_history_and_parent_then_retries() {
    for checkpointed in [false, true] {
        let (client, name, store) = database().await;
        let (parent, _) = retained_fixture(&store, checkpointed).await;
        let before = support::all_rows(&store).await;
        client.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["commitTransaction"],"errorCode":13}}).await.unwrap();
        let result = store.recover_runtime_loss(loss(&parent)).await;
        client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
        assert_eq!(support::all_rows(&store).await, before);
        support::observe_transaction_cleanup(&store).await;
        assert_eq!(support::all_rows(&store).await, before);
        assert_eq!(
            store.recover_runtime_loss(loss(&parent)).await.unwrap(),
            if checkpointed {
                SandboxRuntimeLossOutcome::RecoveryScheduled
            } else {
                SandboxRuntimeLossOutcome::Failed
            }
        );
        client.database(&name).drop().await.unwrap();
    }
}
