use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn writer_parent_fence_invalidates_old_plan_and_committed_hold_denies_writers() {
    use crate::store::ledger_maintenance_writer::fence_writer;
    use layer2_core::sandbox::ledger_maintenance::{LedgerMaintenanceRefusal, LedgerWriterClass};
    let (client, name, store) = database().await;
    let mut request = fixture(&store, false).await;
    let id = request.accepted.plan().source.sandbox.id.clone();
    let mut writer = client.start_session().await.unwrap();
    start_transaction(&mut writer).await.unwrap();
    fence_writer(&store, &id, LedgerWriterClass::Creation, None, &mut writer)
        .await
        .unwrap_or_else(|_| panic!("fixture writer fence failed"));
    writer.commit_transaction().await.unwrap();
    let before = documents(&store).await;
    assert_eq!(
        store
            .admit_ledger_maintenance(ENABLED, request.clone())
            .await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    assert_eq!(documents(&store).await, before);
    let mut plan = request.accepted.plan().clone();
    plan.source = store
        .inspect_ledger_maintenance(&request.caller, &id)
        .await
        .unwrap()
        .source;
    request.accepted = artifact(&plan);
    store
        .admit_ledger_maintenance(ENABLED, request)
        .await
        .unwrap();
    let before = documents(&store).await;
    for class in [
        LedgerWriterClass::Creation,
        LedgerWriterClass::Recorder,
        LedgerWriterClass::Purge,
    ] {
        let mut writer = client.start_session().await.unwrap();
        start_transaction(&mut writer).await.unwrap();
        assert!(matches!(
            fence_writer(&store, &id, class, None, &mut writer).await,
            Err(ApplyError::Domain(Domain::WriterRefused(
                LedgerMaintenanceRefusal::Held
            )))
        ));
        writer.abort_transaction().await.unwrap();
    }
    assert_eq!(documents(&store).await, before);
    assert_eq!(
        store
            .agent_command_orders
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn sidecar_drift_and_missing_audit_refuse_without_writes() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    store
        .agents
        .update_one(doc! {}, doc! {"$set":{"fixture_sidecar_revision":1_i64}})
        .await
        .unwrap();
    let before = documents(&store).await;
    assert_eq!(
        store
            .admit_ledger_maintenance(ENABLED, request.clone())
            .await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    assert_eq!(documents(&store).await, before);
    store
        .agents
        .update_one(doc! {}, doc! {"$unset":{"fixture_sidecar_revision":""}})
        .await
        .unwrap();
    store
        .admit_ledger_maintenance(ENABLED, request.clone())
        .await
        .unwrap();
    transaction::audit_collection(&store)
        .delete_many(doc! {})
        .await
        .unwrap();
    let before = documents(&store).await;
    assert_eq!(
        store.admit_ledger_maintenance(ENABLED, request).await,
        Err(Domain::Corrupt)
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set with test commands"]
async fn real_ambiguous_success_uses_reconciliation_and_writes_one_audit() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    client.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":5},
        "data":{"failCommands":["commitTransaction"], "writeConcernError":{
            "code":50, "codeName":"MaxTimeMSExpired", "errmsg":"injected ambiguous maintenance commit",
        }},
    }).await.unwrap();
    let outcome = store
        .admit_ledger_maintenance(ENABLED, request.clone())
        .await;
    client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    let state = outcome.unwrap();
    assert_eq!(state.accepted_plan_bytes, request.accepted.original_bytes());
    assert_eq!(
        transaction::audit_collection(&store)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let before = documents(&store).await;
    assert_eq!(
        store
            .admit_ledger_maintenance(ENABLED, request)
            .await
            .unwrap(),
        state
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn independent_lane_write_after_snapshot_conflicts_before_hold_commit() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    super::super::ledger_maintenance_snapshot::load(
        &store,
        &request.caller,
        &request.accepted.plan().source.sandbox.id,
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("fixture snapshot failed"));
    other
        .agent_command_orders
        .update_one(doc! {}, doc! {"$inc":{"next_fence":1_i64}})
        .await
        .unwrap();
    let before = documents(&store).await;
    assert!(
        matches!(transaction::apply(&store, &Change::Admit(ENABLED, request.clone()), &mut session).await,
        Err(ApplyError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(documents(&store).await, before);
    assert_eq!(
        other.admit_ledger_maintenance(ENABLED, request).await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set with test commands"]
async fn exhausted_unknown_commits_refuse_without_restarting_the_logical_operation() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    let before = documents(&store).await;
    client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":5},
            "data":{"failCommands":["commitTransaction"], "errorCode":251,
                "errorLabels":["UnknownTransactionCommitResult"]},
        })
        .await
        .unwrap();
    let outcome = store
        .admit_ledger_maintenance(ENABLED, request.clone())
        .await;
    client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    assert_eq!(outcome, Err(Domain::PendingCommit));
    assert_eq!(documents(&store).await, before);
    let admitted = store
        .admit_ledger_maintenance(ENABLED, request.clone())
        .await
        .unwrap();
    assert_eq!(
        admitted.accepted_plan_bytes,
        request.accepted.original_bytes()
    );
    assert_eq!(
        admitted.record.binding.maintenance_id,
        request.accepted.plan().binding.maintenance_id
    );
    assert_eq!(
        transaction::audit_collection(&store)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn foreign_and_capabilityless_admission_refuse_but_operator_is_audited() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    let before = documents(&store).await;
    for caller in [
        Caller::new(
            SubjectId("unrelated-fixture-principal".into()),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        Caller::new(request.caller.subject.clone(), Authorization::new([])),
    ] {
        let mut rejected = request.clone();
        rejected.caller = caller;
        assert_eq!(
            store.admit_ledger_maintenance(ENABLED, rejected).await,
            Err(Domain::Refused(Refusal::NotFound))
        );
        assert_eq!(documents(&store).await, before);
    }
    let mut administrative = request;
    administrative.caller = Caller::new(
        SubjectId("fixture-operator".into()),
        Authorization::new([Capability::ControlAnySandbox]),
    );
    assert!(store
        .admit_ledger_maintenance(ENABLED, administrative)
        .await
        .is_ok());
    let audit = transaction::audit_collection(&store)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(audit.get_str("authority").unwrap(), "operator");
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn malformed_binary_and_cross_owner_children_fail_closed_without_writes() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    store
        .admit_ledger_maintenance(ENABLED, request.clone())
        .await
        .unwrap();
    let parent = store.sandboxes.find_one(doc! {}).await.unwrap().unwrap();
    let mut invalid = parent.get_document(FIELD).unwrap().clone();
    invalid.insert("accepted_plan_bytes", Bson::Array(vec![Bson::Int32(1)]));
    store
        .sandboxes
        .update_one(doc! {}, doc! {"$set":{FIELD:invalid}})
        .await
        .unwrap();
    let before = documents(&store).await;
    assert_eq!(
        store
            .admit_ledger_maintenance(ENABLED, request.clone())
            .await,
        Err(Domain::Corrupt)
    );
    assert_eq!(documents(&store).await, before);
    store.sandboxes.replace_one(doc! {}, parent).await.unwrap();
    store
        .agents
        .update_one(
            doc! {},
            doc! {"$set":{
                "owner":"foreign-fixture-principal", "record.owner":"foreign-fixture-principal",
            }},
        )
        .await
        .unwrap();
    let before = documents(&store).await;
    assert!(matches!(
        store
            .inspect_ledger_maintenance(&request.caller, &request.accepted.plan().source.sandbox.id)
            .await,
        Err(Domain::Corrupt)
    ));
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}
