//! Real aggregate/event ports: no provider or production access.
use super::*;
use crate::store::ledger_maintenance_publication::test_support as maintenance;
use crate::store::ledger_maintenance_test_support::{self as support, all_rows, NOW};
use crate::store::sandbox_maintenance::mongo_integration_tests::database;
use layer2_core::sandbox::ledger_maintenance_plan::LedgerMaintenanceSnapshotV1;
use layer2_core::sandbox::store::{
    AgentStore, SandboxEventStore, SandboxStore, SandboxTransaction, SandboxTransactionStore,
    WorkspaceStore, WorktreeStore,
};
use layer2_core::sandbox::{SandboxEventKind, SandboxStatus};
use layer2_core::{AgentId, WorktreeId};

#[path = "ledger_maintenance_aggregate_race_tests.rs"]
mod races;

fn mutation(source: &LedgerMaintenanceSnapshotV1, kind: u8) -> SandboxTransaction {
    match kind {
        0 => {
            let mut worktree = source.worktrees[0].clone();
            let mut agent = source.agents[0].clone();
            worktree.id = WorktreeId::generate();
            agent.id = AgentId::generate();
            agent.worktree_id = worktree.id.clone();
            worktree.agent_id = Some(agent.id.clone());
            worktree.name = "wt-2".into();
            worktree.path = "/home/cma/fixture/.worktrees/wt-2".into();
            worktree.branch = "cma/wt-2".into();
            let event = SandboxEventKind::AgentCreated {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: worktree.id.clone(),
                agent_id: agent.id.clone(),
            };
            SandboxTransaction::CreateWorktreeAndAgent {
                worktree: Box::new(worktree),
                agent: Box::new(agent),
                event_at: NOW.into(),
                event,
            }
        }
        1 => {
            let mut parent = source.sandbox.clone();
            parent.begin_retirement(NOW.into()).unwrap();
            SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                expected_revision: source.sandbox.revision,
                sandbox: Box::new(parent),
                event_at: NOW.into(),
                event: SandboxEventKind::StatusChanged {
                    from: SandboxStatus::Active,
                    to: SandboxStatus::Retiring,
                },
            }
        }
        2 => {
            let mut workspace = source.workspaces[0].clone();
            let expected_revision = workspace.revision;
            workspace.revision += 1;
            let event = SandboxEventKind::WorkspaceReady {
                workspace_id: workspace.id.clone(),
                resolved_commit: workspace.resolved_commit.clone().unwrap(),
            };
            SandboxTransaction::SaveWorkspaceAndAppend {
                expected_revision,
                workspace: Box::new(workspace),
                event_at: NOW.into(),
                event,
            }
        }
        _ => unreachable!(),
    }
}

async fn assert_saved(store: &MongoSandboxStore, mutation: &SandboxTransaction) {
    let sandbox_id = match mutation {
        SandboxTransaction::CreateWorktreeAndAgent {
            worktree, agent, ..
        } => {
            assert_eq!(
                store.get_worktree(&worktree.id).await.unwrap().as_ref(),
                Some(worktree.as_ref())
            );
            assert!(store.get_agent(&agent.id).await.unwrap().as_ref() == Some(agent.as_ref()));
            &worktree.sandbox_id
        }
        SandboxTransaction::SaveSandboxAndAppend { sandbox, .. } => {
            assert_eq!(
                store.get_sandbox(&sandbox.id).await.unwrap().as_ref(),
                Some(sandbox.as_ref())
            );
            &sandbox.id
        }
        SandboxTransaction::SaveWorkspaceAndAppend { workspace, .. } => {
            assert!(
                store.get_workspace(&workspace.id).await.unwrap().as_ref()
                    == Some(workspace.as_ref())
            );
            &workspace.sandbox_id
        }
    };
    assert_eq!(
        store
            .sandbox_events(sandbox_id, 0, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    let parent = store.get_sandbox(sandbox_id).await.unwrap().unwrap();
    assert_eq!(maintenance::fence_value(store, &parent).await, 1);
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and aggregate hooks"]
async fn all_aggregate_ports_succeed_unheld_and_refuse_both_holds_after_reopen() {
    for kind in 0..3 {
        for hold in [0, 1, 2] {
            let (client, name, store) = database().await;
            let input = support::fixture(&store, true).await;
            let source = input.accepted.plan().source.clone();
            let proposed = mutation(&source, kind);
            if hold != 0 {
                maintenance::hold(&store, &source.sandbox, hold == 1).await;
            }
            let before = all_rows(&store).await;
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let result = reopened.commit(proposed.clone()).await;
            if hold == 0 {
                result.unwrap();
                assert_saved(&reopened, &proposed).await;
            } else {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_eq!(all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and aggregate hooks"]
async fn missing_hold_audit_refuses_all_aggregate_ports_without_writes() {
    for kind in 0..3 {
        for v1 in [false, true] {
            let (client, name, store) = database().await;
            let source = support::fixture(&store, true)
                .await
                .accepted
                .plan()
                .source
                .clone();
            maintenance::hold(&store, &source.sandbox, v1).await;
            let audits = if v1 {
                crate::store::ledger_maintenance_transaction::audit_collection(&store)
            } else {
                crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
            };
            audits.delete_many(doc! {}).await.unwrap();
            let before = all_rows(&store).await;
            assert!(matches!(
                store.commit(mutation(&source, kind)).await,
                Err(SandboxStoreError::Corrupt(_))
            ));
            assert_eq!(all_rows(&store).await, before);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and aggregate hooks"]
async fn missing_or_forged_custody_refuses_without_partial_event_or_children() {
    for kind in 0..3 {
        for missing in [false, true] {
            let (client, name, store) = database().await;
            let source = support::fixture(&store, true)
                .await
                .accepted
                .plan()
                .source
                .clone();
            let mut proposed = mutation(&source, kind);
            if missing {
                store.sandboxes.delete_many(doc! {}).await.unwrap();
            } else {
                let foreign = SubjectId("foreign-fixture".into());
                match &mut proposed {
                    SandboxTransaction::CreateWorktreeAndAgent {
                        worktree, agent, ..
                    } => {
                        worktree.owner = foreign.clone();
                        agent.owner = foreign;
                    }
                    SandboxTransaction::SaveSandboxAndAppend { sandbox, .. } => {
                        sandbox.owner = foreign
                    }
                    SandboxTransaction::SaveWorkspaceAndAppend { workspace, .. } => {
                        workspace.owner = foreign
                    }
                }
            }
            let before = all_rows(&store).await;
            assert_eq!(
                store.commit(proposed).await,
                Err(if missing {
                    SandboxStoreError::NotFound
                } else {
                    SandboxStoreError::Conflict
                })
            );
            assert_eq!(all_rows(&store).await, before);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and aggregate hooks"]
async fn event_failure_rolls_back_aggregate_children_metering_and_parent_fence() {
    for kind in 0..3 {
        let (client, name, store) = database().await;
        let source = support::fixture(&store, true)
            .await
            .accepted
            .plan()
            .source
            .clone();
        let proposed = mutation(&source, kind);
        client.database(&name).run_command(doc! {"collMod":store.sandbox_events.name(),"validator":{"reject_fixture":{"$exists":true}},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
        let before = all_rows(&store).await;
        assert!(matches!(
            store.commit(proposed.clone()).await,
            Err(SandboxStoreError::Unavailable(_))
        ));
        assert_eq!(all_rows(&store).await, before);
        client
            .database(&name)
            .run_command(doc! {"collMod":store.sandbox_events.name(),"validator":{}})
            .await
            .unwrap();
        store.commit(proposed.clone()).await.unwrap();
        assert_saved(&store, &proposed).await;
        client.database(&name).drop().await.unwrap();
    }
}
