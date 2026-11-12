use super::*;
use crate::store::sandbox_mongo::is_transient;

#[tokio::test]
#[ignore = "requires an owned disposable replica set and worktree recovery hooks"]
async fn hold_invalidates_old_recovery_transaction_and_fresh_retry() {
    use crate::store::agent_worktree_recovery_records::ApplyError;
    for v1 in [false, true] {
        let (client, name, store, parent, original) = retained_fixture().await;
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
        assert!(matches!(crate::store::agent_worktree_recovery::apply(
            &store, &original.id, &owner(), "recover", &WorktreeId::generate(), NOW, &mut session
        ).await, Err(ApplyError::Driver(error)) if is_transient(&error)));
        let _ = session.abort_transaction().await;
        assert_eq!(
            recover(&store, &original, "recover").await,
            Err(AgentControlStoreError::Conflict)
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and worktree recovery hooks"]
async fn recovery_invalidates_old_admission_snapshot_and_original_plan() {
    use crate::store::ledger_maintenance::ApplyError as HoldError;
    use crate::store::ledger_maintenance_pre_exclusion::Change;
    use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
    use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
    use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;
    let (client, name, store, parent, original) = retained_fixture().await;
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
    recover(&other, &original, "recover").await.unwrap();
    let written = support::all_rows(&store).await;
    assert!(
        matches!(crate::store::ledger_maintenance_pre_exclusion_transaction::apply(
        &store, &policy, &Change::Admit(input.clone()), &mut session
    ).await, Err(HoldError::Driver(error)) if is_transient(&error))
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
