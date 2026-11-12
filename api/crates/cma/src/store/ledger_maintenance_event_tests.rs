use super::*;
use crate::store::ledger_maintenance_publication::test_support as maintenance;
use crate::store::ledger_maintenance_test_support::{self as support, all_rows, NOW};
use crate::store::sandbox_maintenance::mongo_integration_tests::database;
use layer2_core::sandbox::store::SandboxStore;

#[path = "ledger_maintenance_event_race_tests.rs"]
mod races;

#[tokio::test]
#[ignore = "requires an owned disposable replica set and standalone event hook"]
async fn standalone_events_succeed_unheld_and_refuse_both_holds_after_reopen() {
    for hold in [0, 1, 2] {
        let (client, name, store) = database().await;
        let parent = support::fixture(&store, true)
            .await
            .accepted
            .plan()
            .source
            .sandbox
            .clone();
        if hold != 0 {
            maintenance::hold(&store, &parent, hold == 1).await;
        }
        let before = all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let result = reopened
            .append_sandbox_event(&parent.id, NOW.into(), SandboxEventKind::Created)
            .await;
        if hold == 0 {
            let event = result.unwrap();
            assert_eq!(event.sandbox_id, parent.id);
            assert_eq!(event.seq, 1);
            assert_eq!(event.appended_at, NOW);
            assert_eq!(event.kind, SandboxEventKind::Created);
            assert_eq!(
                store.sandbox_events(&parent.id, 0, 100).await.unwrap(),
                vec![event]
            );
            assert_eq!(
                store.get_sandbox(&parent.id).await.unwrap(),
                Some(parent.clone())
            );
            assert_eq!(maintenance::fence_value(&store, &parent).await, 1);
        } else {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and standalone event hook"]
async fn standalone_event_refuses_missing_parent_and_incomplete_hold_authority() {
    for scenario in 0..3 {
        let (client, name, store) = database().await;
        let parent = support::fixture(&store, true)
            .await
            .accepted
            .plan()
            .source
            .sandbox
            .clone();
        if scenario == 0 {
            store.sandboxes.delete_many(doc! {}).await.unwrap();
        } else {
            maintenance::hold(&store, &parent, scenario == 1).await;
            let audits = if scenario == 1 {
                crate::store::ledger_maintenance_transaction::audit_collection(&store)
            } else {
                crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
            };
            audits.delete_many(doc! {}).await.unwrap();
        }
        let before = all_rows(&store).await;
        let result = store
            .append_sandbox_event(&parent.id, NOW.into(), SandboxEventKind::Created)
            .await;
        if scenario == 0 {
            assert_eq!(result, Err(SandboxStoreError::NotFound));
        } else {
            assert!(matches!(result, Err(SandboxStoreError::Corrupt(_))));
        }
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and standalone event hook"]
async fn event_insertion_failure_rolls_back_parent_sequence_fence_and_usage() {
    let (client, name, store) = database().await;
    let parent = support::fixture(&store, true)
        .await
        .accepted
        .plan()
        .source
        .sandbox
        .clone();
    client.database(&name).run_command(doc! {"collMod":store.sandbox_events.name(),"validator":{"reject_fixture":{"$exists":true}},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    let before = all_rows(&store).await;
    assert!(matches!(
        store
            .append_sandbox_event(&parent.id, NOW.into(), SandboxEventKind::Created)
            .await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    assert_eq!(all_rows(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":store.sandbox_events.name(),"validator":{}})
        .await
        .unwrap();
    assert_eq!(
        store
            .append_sandbox_event(&parent.id, NOW.into(), SandboxEventKind::Created)
            .await
            .unwrap()
            .seq,
        1
    );
    assert_eq!(maintenance::fence_value(&store, &parent).await, 1);
    client.database(&name).drop().await.unwrap();
}
