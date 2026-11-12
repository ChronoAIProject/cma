use super::*;
use crate::store::sandbox_mongo::is_transient;

#[tokio::test]
#[ignore = "requires an owned disposable replica set and reconciliation hooks"]
async fn hold_commit_invalidates_old_reconciliation_snapshot_and_fresh_retry() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let (parent, _) = retained_fixture(&store, true).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        store
            .sandboxes
            .find_one(doc! {"_id":parent.id.as_str()})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap();
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        maintenance::hold(&other, &parent, v1).await;
        let before = support::all_rows(&store).await;
        assert!(
            matches!(transaction::apply(&store, &loss(&parent), &mut session).await,
            Err(ApplyError::Driver(error)) if is_transient(&error))
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            store.recover_runtime_loss(loss(&parent)).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and reconciliation hooks"]
async fn reconciliation_commit_invalidates_admission_snapshot_and_original_plan() {
    use crate::store::ledger_maintenance::ApplyError as HoldError;
    use crate::store::ledger_maintenance_pre_exclusion::Change;
    use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
    use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
    use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;
    let (client, name, store) = database().await;
    let (parent, _) = retained_fixture(&store, true).await;
    let (policy, input) = support::pre_exclusion(&store, &parent, support::NOW).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    crate::store::ledger_maintenance_snapshot::load_pre_exclusion(
        &store,
        &input.caller,
        &parent.id,
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("fixture snapshot failed"));
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        other.recover_runtime_loss(loss(&parent)).await.unwrap(),
        SandboxRuntimeLossOutcome::RecoveryScheduled
    );
    let written = support::all_rows(&store).await;
    assert!(
        matches!(crate::store::ledger_maintenance_pre_exclusion_transaction::apply(
        &store, &policy, &Change::Admit(input.clone()), &mut session).await,
        Err(HoldError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(
        store.admit_pre_exclusion(policy, input).await,
        Err(LedgerMaintenanceStoreError::Refused(
            LedgerMaintenanceAdmissionRefusal::Conflict
        ))
    );
    assert_eq!(support::all_rows(&store).await, written);
    client.database(&name).drop().await.unwrap();
}
