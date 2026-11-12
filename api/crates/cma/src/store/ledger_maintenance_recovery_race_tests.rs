use super::*;
use crate::store::sandbox_mongo::is_transient;

#[tokio::test]
#[ignore = "requires an owned disposable replica set and recovery hooks"]
async fn held_commit_invalidates_a_reopen_snapshot_and_fresh_recovery_refuses() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let request = reopen_fixture(&store, "sbx_recovery_race").await;
        let parent = &request.snapshot.retained.sandbox;
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
        maintenance::hold(&other, parent, v1).await;
        let before = support::all_rows(&store).await;
        assert!(
            matches!(transaction::apply(&store, &request, &mut session).await,
            Err(ApplyError::Driver(error)) if is_transient(&error))
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            store.admit_retained_reopen(request).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and recovery hooks"]
async fn recovery_commit_invalidates_admission_snapshot_and_original_plan() {
    use crate::store::ledger_maintenance::ApplyError as HoldError;
    use crate::store::ledger_maintenance_pre_exclusion::Change;
    for retained in [false, true] {
        let (client, name, store) = database().await;
        let request = Recovery::fixture(&store, retained).await;
        let (policy, input) = support::pre_exclusion(&store, request.parent(), support::NOW).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        crate::store::ledger_maintenance_snapshot::load_pre_exclusion(
            &store,
            &input.caller,
            &request.parent().id,
            &mut session,
        )
        .await
        .unwrap_or_else(|_| panic!("fixture snapshot failed"));
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        request.apply(&other).await.unwrap();
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
}
