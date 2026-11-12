use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable replica set and standalone event hook"]
async fn committed_hold_invalidates_an_old_event_snapshot_and_refuses_fresh_append() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let parent = support::fixture(&store, true)
            .await
            .accepted
            .plan()
            .source
            .sandbox
            .clone();
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
        let before = all_rows(&store).await;
        assert!(
            matches!(append(&store, &mut session, &parent.id, NOW.into(), SandboxEventKind::Created).await,
            Err(TransactionEventError::Driver(error)) if is_transient(&error))
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            store
                .append_sandbox_event(&parent.id, NOW.into(), SandboxEventKind::Created)
                .await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and standalone event hook"]
async fn event_commit_invalidates_old_hold_snapshot_without_changing_public_revision() {
    use crate::store::ledger_maintenance::ApplyError as HoldError;
    use crate::store::ledger_maintenance_pre_exclusion::Change;
    use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
    use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
    use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;
    let (client, name, store) = database().await;
    let parent = support::fixture(&store, true)
        .await
        .accepted
        .plan()
        .source
        .sandbox
        .clone();
    let (policy, input) = support::pre_exclusion(&store, &parent, NOW).await;
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
        .append_sandbox_event(&parent.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
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
