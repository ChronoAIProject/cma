//! Exercise the public save ports against an owned disposable replica set.
use super::*;
use crate::store::ledger_maintenance_test_support::{self as support, all_rows, NOW};
use crate::store::sandbox_maintenance::mongo_integration_tests::database;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionPolicy;
use layer2_core::sandbox::ledger_maintenance_plan::LedgerMaintenanceSnapshotV1;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStore;
use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;
use layer2_core::sandbox::store::{AgentStore, WorkspaceStore};

#[path = "ledger_maintenance_child_save_race_tests.rs"]
mod races;

fn next(source: &LedgerMaintenanceSnapshotV1) -> (Agent, Workspace) {
    let mut agent = source.agents[0].clone();
    let mut workspace = source.workspaces[0].clone();
    agent.revision += 1;
    workspace.revision += 1;
    (agent, workspace)
}

fn mutation(agent: bool, next: &(Agent, Workspace)) -> Mutation<'_> {
    if agent {
        Mutation::Agent(&next.0)
    } else {
        Mutation::Workspace(&next.1)
    }
}

async fn port(
    store: &MongoSandboxStore,
    agent: bool,
    next: &(Agent, Workspace),
) -> Result<(), SandboxStoreError> {
    if agent {
        store.save_agent(next.0.revision - 1, next.0.clone()).await
    } else {
        store
            .save_workspace(next.1.revision - 1, next.1.clone())
            .await
    }
}

async fn assert_saved(store: &MongoSandboxStore, agent: bool, next: &(Agent, Workspace)) {
    if agent {
        let actual = store.get_agent(&next.0.id).await.unwrap().unwrap();
        assert_eq!(actual.revision, next.0.revision, "saved Agent revision");
        assert!(actual == next.0);
    } else {
        let actual = store.get_workspace(&next.1.id).await.unwrap().unwrap();
        assert_eq!(actual.revision, next.1.revision, "saved Workspace revision");
        assert!(actual == next.1);
    }
    let parent = store.sandboxes.find_one(doc! {}).await.unwrap().unwrap();
    assert_eq!(
        parent.get_i64("ledger_writer_admission_fence_v1").unwrap(),
        1
    );
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and child-save hooks"]
async fn positive_saves_and_both_hold_formats_through_reopened_ports() {
    for agent in [false, true] {
        for hold in [0, 1, 2] {
            let (client, name, store) = database().await;
            let input = support::fixture(&store, true).await;
            let source = input.accepted.plan().source.clone();
            let next = next(&source);
            if hold == 1 {
                store
                    .admit_ledger_maintenance(
                        LedgerMaintenanceAdmissionPolicy { enabled: true },
                        input,
                    )
                    .await
                    .unwrap();
            } else if hold == 2 {
                let (policy, input) = support::pre_exclusion(&store, &source.sandbox, NOW).await;
                store.admit_pre_exclusion(policy, input).await.unwrap();
            }
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let before = all_rows(&store).await;
            let result = port(&reopened, agent, &next).await;
            if hold == 0 {
                result.unwrap();
                assert_saved(&reopened, agent, &next).await;
                assert_ne!(all_rows(&store).await, before);
            } else {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_eq!(all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and child-save hooks"]
async fn missing_audits_and_malformed_holds_refuse_every_child_port() {
    for agent in [false, true] {
        for v1 in [false, true] {
            for malformed in [false, true] {
                let (client, name, store) = database().await;
                let input = support::fixture(&store, true).await;
                let source = input.accepted.plan().source.clone();
                let (field, audit) = if v1 {
                    store
                        .admit_ledger_maintenance(
                            LedgerMaintenanceAdmissionPolicy { enabled: true },
                            input,
                        )
                        .await
                        .unwrap();
                    (
                        crate::store::ledger_maintenance_records::FIELD,
                        crate::store::ledger_maintenance_transaction::audit_collection(&store),
                    )
                } else {
                    let (policy, input) =
                        support::pre_exclusion(&store, &source.sandbox, NOW).await;
                    store.admit_pre_exclusion(policy, input).await.unwrap();
                    (crate::store::ledger_maintenance_pre_exclusion_records::FIELD,
                     crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store))
                };
                if malformed {
                    store
                        .sandboxes
                        .update_one(doc! {}, doc! {"$set":{field:"malformed-fixture"}})
                        .await
                        .unwrap();
                } else {
                    audit.delete_many(doc! {}).await.unwrap();
                }
                let before = all_rows(&store).await;
                assert!(matches!(
                    port(&store, agent, &next(&source)).await,
                    Err(SandboxStoreError::Corrupt(_))
                ));
                assert_eq!(all_rows(&store).await, before);
                client.database(&name).drop().await.unwrap();
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and child-save hooks"]
async fn retargeted_missing_and_foreign_custody_cannot_choose_an_unheld_parent() {
    for agent in [false, true] {
        for scenario in 0..4 {
            let (client, name, store) = database().await;
            let input = support::fixture(&store, true).await;
            let mut next = next(&input.accepted.plan().source);
            match scenario {
                0 => {
                    next.0.sandbox_id = SandboxId::generate();
                    next.1.sandbox_id = SandboxId::generate();
                }
                1 => {
                    store.sandboxes.delete_many(doc! {}).await.unwrap();
                }
                2 => {
                    let mut foreign = input.accepted.plan().source.sandbox.clone();
                    foreign.owner = SubjectId("foreign-fixture".into());
                    store
                        .sandboxes
                        .replace_one(
                            doc! {},
                            crate::store::sandboxes::sandbox_document(&foreign, 0).unwrap(),
                        )
                        .await
                        .unwrap();
                }
                3 => {
                    next.0.created_at = support::NEXT.into();
                    next.1.created_at = support::NEXT.into();
                }
                _ => unreachable!(),
            }
            let before = all_rows(&store).await;
            assert_eq!(
                port(&store, agent, &next).await,
                Err(if scenario == 1 {
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
#[ignore = "requires an owned disposable MongoDB replica set and child-save hooks"]
async fn existing_cleanup_cas_predicates_still_refuse_and_roll_back_parent_fence() {
    for (agent, field) in [
        (true, "channel_reuse_cleanup_execution"),
        (true, "channel_history_seal"),
        (false, "channel_reuse_cleanup_execution"),
        (false, "owned_workspace_cleanup_admission_v1"),
    ] {
        let (client, name, store) = database().await;
        let input = support::fixture(&store, true).await;
        let next = next(&input.accepted.plan().source);
        let collection = if agent {
            &store.agents
        } else {
            &store.workspaces
        };
        collection
            .update_one(doc! {}, doc! {"$set":{field: "fixture-reservation"}})
            .await
            .unwrap();
        let before = all_rows(&store).await;
        assert_eq!(
            port(&store, agent, &next).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(all_rows(&store).await, before);
        collection
            .update_one(doc! {}, doc! {"$unset":{field:""}})
            .await
            .unwrap();
        port(&store, agent, &next).await.unwrap();
        assert_saved(&store, agent, &next).await;
        client.database(&name).drop().await.unwrap();
    }
}
