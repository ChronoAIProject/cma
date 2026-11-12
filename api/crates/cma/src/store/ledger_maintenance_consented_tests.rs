//! Consented revival preserves immutable replay and fences only fresh effects.
use super::*;
use crate::store::ledger_maintenance_publication::test_support as maintenance;
use crate::store::ledger_maintenance_test_support as support;

#[path = "ledger_maintenance_consented_race_tests.rs"]
mod races;

async fn retained_fixture() -> (
    mongodb::Client,
    String,
    std::sync::Arc<MongoSandboxStore>,
    AgentAccessRecord,
    Sandbox,
) {
    let (client, name, store, record) = granted_fixture().await;
    let mut current = parent(&store, &record).await;
    // The access fixture already binds home_recovery to its retained volume.
    // Keep that original identity instead of replacing its attachment.
    assert!(current.lifecycle.home.is_some());
    current.lifecycle.suspended_runtime = Some(Box::new(record.requester.runtime.clone()));
    current.revision += 1;
    save_parent(&store, &current).await;
    (client, name, store, record, current)
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and consented revival hook"]
async fn fresh_consented_revival_succeeds_unheld_and_refuses_both_holds_after_reopen() {
    for hold in [0, 1, 2] {
        let (client, name, store, record, current) = retained_fixture().await;
        let fence = maintenance::fence_value(&store, &current).await;
        if hold != 0 {
            maintenance::hold(&store, &current, hold == 1).await;
        }
        let before = support::all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let result = reopened
            .schedule_access_request_revival(record.lease.as_ref().unwrap())
            .await;
        if hold == 0 {
            assert_eq!(result.unwrap(), SandboxMaintenanceOutcome::Scheduled);
            let saved = parent(&store, &record).await;
            assert_eq!(saved.status, SandboxStatus::Reviving);
            assert_eq!(saved.revision, current.revision + 1);
            assert_eq!(maintenance::fence_value(&store, &saved).await, fence + 1);
            let recorded = store
                .read_access_request(&record.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                recorded.progress.revive_operation_id.as_deref(),
                Some(
                    saved
                        .lifecycle
                        .operation
                        .as_ref()
                        .unwrap()
                        .authority()
                        .operation_id
                        .as_str()
                )
            );
            assert_ne!(support::all_rows(&store).await, before);
        } else {
            assert_eq!(result, Err(AgentAccessError::Conflict));
            assert_eq!(support::all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and consented revival hook"]
async fn recorded_revival_remains_read_only_while_either_hold_is_present() {
    for v1 in [false, true] {
        let (client, name, store, record, _) = retained_fixture().await;
        let lease = record.lease.as_ref().unwrap();
        assert_eq!(
            store.schedule_access_request_revival(lease).await.unwrap(),
            SandboxMaintenanceOutcome::Scheduled
        );
        let current = parent(&store, &record).await;
        maintenance::hold(&store, &current, v1).await;
        let before = support::all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(
            reopened
                .schedule_access_request_revival(lease)
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::AlreadyScheduled
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and consented revival hook"]
async fn missing_hold_audit_and_stale_consent_lease_refuse_without_writes() {
    for v1 in [false, true] {
        let (client, name, store, record, current) = retained_fixture().await;
        let mut stale = record.lease.clone().unwrap();
        stale.token = "stale-fixture-lease".into();
        let before = support::all_rows(&store).await;
        assert_eq!(
            store.schedule_access_request_revival(&stale).await,
            Err(AgentAccessError::Conflict)
        );
        assert_eq!(support::all_rows(&store).await, before);
        maintenance::hold(&store, &current, v1).await;
        let audit = if v1 {
            crate::store::ledger_maintenance_transaction::audit_collection(&store)
        } else {
            crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
        };
        audit.delete_many(doc! {}).await.unwrap();
        let before = support::all_rows(&store).await;
        assert_eq!(
            store
                .schedule_access_request_revival(record.lease.as_ref().unwrap())
                .await,
            Err(AgentAccessError::Unavailable)
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and consented revival hook"]
async fn failed_access_receipt_write_rolls_back_parent_event_and_all_fences() {
    let (client, name, store, record, current) = retained_fixture().await;
    client.database(&name).run_command(doc! {"collMod":store.agent_access_requests.name(),"validator":{"reject_fixture":{"$exists":true}},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    let before = support::all_rows(&store).await;
    let lease = record.lease.as_ref().unwrap();
    assert_eq!(
        store.schedule_access_request_revival(lease).await,
        Err(AgentAccessError::Unavailable)
    );
    assert_eq!(support::all_rows(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":store.agent_access_requests.name(),"validator":{}})
        .await
        .unwrap();
    assert_eq!(
        store.schedule_access_request_revival(lease).await.unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_eq!(parent(&store, &record).await.revision, current.revision + 1);
    client.database(&name).drop().await.unwrap();
}
