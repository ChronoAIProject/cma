//! Both transaction orderings use independent store instances and snapshots.
use super::*;
use crate::store::sandbox_mongo::{is_transient, start_transaction};

#[tokio::test]
#[ignore = "requires an owned disposable replica set and aggregate hooks"]
async fn committed_hold_invalidates_each_old_aggregate_snapshot_and_fresh_retry() {
    for kind in 0..3 {
        for v1 in [false, true] {
            let (client, name, store) = database().await;
            let source = support::fixture(&store, true).await.accepted.plan().source;
            let proposed = mutation(&source, kind);
            let mut session = client.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            store
                .sandboxes
                .find_one(doc! {"_id":source.sandbox.id.as_str()})
                .session(&mut session)
                .await
                .unwrap()
                .unwrap();
            let other = MongoSandboxStore::open(&client, &name).await.unwrap();
            maintenance::hold(&other, &source.sandbox, v1).await;
            let before = all_rows(&store).await;
            assert!(
                matches!(crate::store::sandbox_transactions::apply_transaction(&store, &mut session, &proposed).await,
                Err(ApplyError::Driver(error)) if is_transient(&error))
            );
            let _ = session.abort_transaction().await;
            assert_eq!(
                store.commit(proposed).await,
                Err(SandboxStoreError::Conflict)
            );
            assert_eq!(all_rows(&store).await, before);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and aggregate hooks"]
async fn each_aggregate_commit_invalidates_old_hold_snapshot_and_original_plan() {
    use crate::store::ledger_maintenance::ApplyError as HoldError;
    use crate::store::ledger_maintenance_pre_exclusion::Change;
    use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
    use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
    use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;
    for kind in 0..3 {
        let (client, name, store) = database().await;
        let source = support::fixture(&store, true).await.accepted.plan().source;
        let (policy, input) = support::pre_exclusion(&store, &source.sandbox, NOW).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        crate::store::ledger_maintenance_snapshot::load_pre_exclusion(
            &store,
            &input.caller,
            &source.sandbox.id,
            &mut session,
        )
        .await
        .unwrap_or_else(|_| panic!("fixture snapshot failed"));
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        other.commit(mutation(&source, kind)).await.unwrap();
        let written = all_rows(&store).await;
        assert!(
            matches!(crate::store::ledger_maintenance_pre_exclusion_transaction::apply(
            &store, &policy, &Change::Admit(input.clone()), &mut session,
        ).await, Err(HoldError::Driver(error)) if is_transient(&error))
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            store.admit_pre_exclusion(policy, input).await,
            Err(LedgerMaintenanceStoreError::Refused(
                LedgerMaintenanceAdmissionRefusal::Conflict
            ))
        );
        assert_eq!(all_rows(&store).await, written);
        client.database(&name).drop().await.unwrap();
    }
}
