//! Real access workflow ports, with complete collection comparisons.
use super::*;
use crate::store::ledger_maintenance_publication::test_support as maintenance;
use crate::store::ledger_maintenance_test_support as support;
use layer2_core::sandbox::store::SandboxStore;
use layer2_core::sandbox::Sandbox;
use std::{sync::Arc, time::Duration};

#[path = "ledger_maintenance_access_race_tests.rs"]
mod races;

async fn fixture(
    paused: bool,
) -> (
    mongodb::Client,
    String,
    Arc<MongoSandboxStore>,
    AgentAccessRecord,
    Sandbox,
) {
    let (client, name, store, record) = if paused {
        crate::store::agent_access_requests::grant::tests::paused_fixture().await
    } else {
        crate::store::agent_access_requests::tests::approved_fixture().await
    };
    let mut parent = store
        .get_sandbox(&record.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    // Preserve the original retained attachment and any recovery binding.
    assert!(parent.lifecycle.home.is_some());
    if parent.runtime.is_none() {
        parent.lifecycle.suspended_runtime = Some(Box::new(record.requester.runtime.clone()));
    }
    parent.revision += 1;
    parent.validate().unwrap();
    store
        .sandboxes
        .update_one(
            doc! {"_id":parent.id.as_str()},
            doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()},
        )
        .await
        .unwrap();
    (client, name, store, record, parent)
}

async fn worker_port(
    store: &MongoSandboxStore,
    original: &AgentAccessRecord,
    kind: u8,
) -> Result<(), AgentAccessError> {
    let lease = original.lease.as_ref().unwrap();
    match kind {
        0 => store.renew_access_lease(lease).await.map(|_| ()),
        1 => store.release_access_request(lease, Duration::ZERO).await,
        2 => store
            .fail_access_request(lease, "fixture_failure")
            .await
            .map(|_| ()),
        3 => store
            .verify_access_request_lineage(original)
            .await
            .map(|_| ()),
        4 => {
            let claimed = store.claim_access_request("fixture-takeover").await?;
            let claimed = claimed.expect("the unleased fixture request must be claimed");
            assert_eq!(claimed.id, original.id);
            assert_ne!(claimed.lease.as_ref().unwrap().token, lease.token);
            Ok(())
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and access hooks"]
async fn worker_record_ports_succeed_unheld_and_refuse_both_holds_after_reopen() {
    for kind in 0..5 {
        for hold in [0, 1, 2] {
            let (client, name, store, original, parent) = fixture(false).await;
            if kind == 4 {
                store
                    .release_access_request(original.lease.as_ref().unwrap(), Duration::ZERO)
                    .await
                    .unwrap();
            }
            if hold != 0 {
                maintenance::hold(&store, &parent, hold == 1).await;
            }
            let before = support::all_rows(&store).await;
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let result = worker_port(&reopened, &original, kind).await;
            if hold == 0 {
                result.unwrap();
                assert_ne!(support::all_rows(&store).await, before);
                let saved = store
                    .read_access_request(&original.id)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(saved.requester, original.requester);
                assert_eq!(saved.decision, original.decision);
                assert_eq!(
                    saved.revision,
                    original.revision
                        + if kind == 3 {
                            0
                        } else if kind == 4 {
                            2
                        } else {
                            1
                        }
                );
            } else {
                assert_eq!(result, Err(AgentAccessError::Conflict));
                assert_eq!(support::all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and access hooks"]
async fn fresh_grants_succeed_unheld_and_both_holds_prevent_scope_or_workflow_changes() {
    for paused in [false, true] {
        for hold in [0, 1, 2] {
            let (client, name, store, original, parent) = fixture(paused).await;
            if hold != 0 {
                maintenance::hold(&store, &parent, hold == 1).await;
            }
            let before = support::all_rows(&store).await;
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let result = if paused {
                reopened
                    .grant_access_request_services(original.lease.as_ref().unwrap())
                    .await
            } else {
                reopened
                    .grant_live_access_request_services(original.lease.as_ref().unwrap())
                    .await
            };
            if hold == 0 {
                let saved = result.unwrap();
                assert_eq!(
                    saved.phase,
                    if paused {
                        AgentAccessPhase::Reviving
                    } else {
                        AgentAccessPhase::Updating
                    }
                );
                assert!(saved.progress.grant_generation_floor.is_some());
                assert_eq!(saved.requester, original.requester);
                assert_eq!(saved.decision, original.decision);
                assert_ne!(support::all_rows(&store).await, before);
            } else {
                assert_eq!(result, Err(AgentAccessError::Conflict));
                assert_eq!(support::all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and access hooks"]
async fn immutable_paused_grant_replays_but_mutating_hot_grant_replay_refuses_while_held() {
    for paused in [false, true] {
        for v1 in [false, true] {
            let (client, name, store, original, _) = fixture(paused).await;
            let lease = original.lease.as_ref().unwrap();
            let granted = if paused {
                store.grant_access_request_services(lease).await.unwrap()
            } else {
                store
                    .grant_live_access_request_services(lease)
                    .await
                    .unwrap()
            };
            let parent = store
                .get_sandbox(&original.requester.sandbox_id)
                .await
                .unwrap()
                .unwrap();
            maintenance::hold(&store, &parent, v1).await;
            let before = support::all_rows(&store).await;
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            if paused {
                assert_eq!(
                    reopened.grant_access_request_services(lease).await.unwrap(),
                    granted
                );
            } else {
                assert_eq!(
                    reopened.grant_live_access_request_services(lease).await,
                    Err(AgentAccessError::Conflict)
                );
            }
            assert_eq!(support::all_rows(&store).await, before);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and access hooks"]
async fn missing_audit_parent_or_recreated_parent_cannot_authorize_lease_writes() {
    for scenario in 0..4 {
        let (client, name, store, original, mut parent) = fixture(false).await;
        if scenario == 0 {
            store.sandboxes.delete_many(doc! {}).await.unwrap();
        } else if scenario == 1 {
            parent.created_at = "1999-01-01T00:00:00Z".into();
            store.sandboxes.update_one(doc! {"_id":parent.id.as_str()},
                doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()}).await.unwrap();
        } else {
            maintenance::hold(&store, &parent, scenario == 2).await;
            let audit = if scenario == 2 {
                crate::store::ledger_maintenance_transaction::audit_collection(&store)
            } else {
                crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
            };
            audit.delete_many(doc! {}).await.unwrap();
        }
        let before = support::all_rows(&store).await;
        assert_eq!(
            store
                .renew_access_lease(original.lease.as_ref().unwrap())
                .await,
            Err(match scenario {
                0 => AgentAccessError::NotFound,
                1 => AgentAccessError::Conflict,
                _ => AgentAccessError::Unavailable,
            })
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and access hooks"]
async fn failed_workflow_write_rolls_back_parent_fence_and_can_retry_exact_request() {
    let (client, name, store, original, parent) = fixture(false).await;
    client.database(&name).run_command(doc! {"collMod":store.agent_access_requests.name(),"validator":{"reject_fixture":{"$exists":true}},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    let before = support::all_rows(&store).await;
    assert_eq!(
        store
            .renew_access_lease(original.lease.as_ref().unwrap())
            .await,
        Err(AgentAccessError::Unavailable)
    );
    assert_eq!(support::all_rows(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":store.agent_access_requests.name(),"validator":{}})
        .await
        .unwrap();
    store
        .renew_access_lease(original.lease.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    assert_eq!(
        store
            .read_access_request(&original.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        original.revision + 1
    );
    client.database(&name).drop().await.unwrap();
}
