use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable replica set and access hooks"]
async fn committed_hold_invalidates_old_lease_transaction_and_fresh_renewal() {
    for v1 in [false, true] {
        let (client, name, store, original, parent) = fixture(false).await;
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
        let mut proposed = original.clone();
        proposed.revision += 1;
        assert!(
            matches!(super::super::super::replace_leased(&store, original.lease.as_ref().unwrap(), original.revision, &proposed, &mut session).await,
            Err(Error::Driver(error)) if is_transient(&error))
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            store
                .renew_access_lease(original.lease.as_ref().unwrap())
                .await,
            Err(AgentAccessError::Conflict)
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and access hooks"]
async fn lease_renewal_invalidates_old_hold_snapshot_and_original_source() {
    use crate::store::ledger_maintenance::ApplyError as HoldError;
    use crate::store::ledger_maintenance_pre_exclusion::Change;
    use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
    use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
    use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;
    let (client, name, store, original, parent) = fixture(false).await;
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
    other
        .renew_access_lease(original.lease.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    let written = support::all_rows(&store).await;
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
    assert_eq!(support::all_rows(&store).await, written);
    client.database(&name).drop().await.unwrap();
}
