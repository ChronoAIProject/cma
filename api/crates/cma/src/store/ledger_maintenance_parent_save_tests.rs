//! The generic parent save needs a guard even for the original V1 hold.
use super::*;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionPolicy;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStore;

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and parent-save hook"]
async fn parent_save_has_a_positive_control_and_both_hold_formats_refuse_without_writes() {
    for hold in [0, 1, 2] {
        let (client, name, store) = database().await;
        let input = support::fixture(&store, true).await;
        let original = input.accepted.plan().source.sandbox.clone();
        if hold == 1 {
            store
                .admit_ledger_maintenance(LedgerMaintenanceAdmissionPolicy { enabled: true }, input)
                .await
                .unwrap();
            assert_eq!(
                store
                    .sandboxes
                    .find_one(doc! {})
                    .await
                    .unwrap()
                    .unwrap()
                    .get_i64("document_schema_version")
                    .unwrap(),
                1
            );
        } else if hold == 2 {
            let (policy, input) = support::pre_exclusion(&store, &original, NOW).await;
            store.admit_pre_exclusion(policy, input).await.unwrap();
        }
        let mut next = original.clone();
        next.name = "renamed-fixture".into();
        next.revision += 1;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let before = all_rows(&store).await;
        let result = reopened.save_sandbox(original.revision, next.clone()).await;
        if hold == 0 {
            result.unwrap();
            assert!(reopened.get_sandbox(&original.id).await.unwrap().unwrap() == next);
            assert_ne!(all_rows(&store).await, before);
        } else {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and parent-save hook"]
async fn parent_save_rejects_missing_audits_and_malformed_holds_without_partial_metering() {
    for v1 in [false, true] {
        for malformed in [false, true] {
            let (client, name, store) = database().await;
            let input = support::fixture(&store, true).await;
            let original = input.accepted.plan().source.sandbox.clone();
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
                let (policy, input) = support::pre_exclusion(&store, &original, NOW).await;
                store.admit_pre_exclusion(policy, input).await.unwrap();
                (records::FIELD, transaction::audit_collection(&store))
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
            let mut next = original.clone();
            next.revision += 1;
            assert!(matches!(
                store.save_sandbox(original.revision, next).await,
                Err(SandboxStoreError::Corrupt(_))
            ));
            assert_eq!(all_rows(&store).await, before);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and parent-save hook"]
async fn metering_failure_rolls_back_parent_save_and_its_admission_fence() {
    let (client, name, store) = database().await;
    let (_, input) = fixture(&store).await;
    let original = input.accepted.plan().source.sandbox.clone();
    let mut next = original.clone();
    next.revision += 1;
    client.database(&name).run_command(doc! {"collMod":store.sandbox_usage.name(),"validator":{"reject_fixture":{"$exists":true}},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    let before = all_rows(&store).await;
    assert!(matches!(
        store.save_sandbox(original.revision, next.clone()).await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    assert_eq!(all_rows(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":store.sandbox_usage.name(),"validator":{}})
        .await
        .unwrap();
    store.save_sandbox(original.revision, next).await.unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and parent-save hook"]
async fn parent_save_commit_invalidates_an_open_maintenance_snapshot() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    let original = input.accepted.plan().source.sandbox.clone();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    ledger_maintenance_snapshot::load_pre_exclusion(
        &store,
        &input.caller,
        &original.id,
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("fixture snapshot failed"));
    let mut next = original.clone();
    next.revision += 1;
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    other.save_sandbox(original.revision, next).await.unwrap();
    let before = all_rows(&store).await;
    assert!(
        matches!(transaction::apply(&store, &policy, &Change::Admit(input.clone()), &mut session).await, Err(ApplyError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(
        store.admit_pre_exclusion(policy, input).await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    assert_eq!(all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and parent-save hook"]
async fn hold_commit_conflicts_with_an_open_parent_save_snapshot() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    let original = input.accepted.plan().source.sandbox.clone();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    ledger_maintenance_snapshot::load_pre_exclusion(
        &store,
        &input.caller,
        &original.id,
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("fixture snapshot failed"));
    store.admit_pre_exclusion(policy, input).await.unwrap();
    let before = all_rows(&store).await;
    assert!(
        matches!(crate::store::ledger_maintenance_writer_ports::parent_save(&store, &original.id, &mut session).await, Err(crate::store::sandbox_events::TransactionEventError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    let mut next = original.clone();
    next.revision += 1;
    assert_eq!(
        store.save_sandbox(original.revision, next).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}
