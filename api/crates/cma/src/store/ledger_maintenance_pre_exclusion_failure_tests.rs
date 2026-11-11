use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn audit_insert_failure_and_abort_restore_parent_source_and_every_fence() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    client.database(&name).run_command(doc! {"collMod":records::AUDITS, "validator":{"reject_fixture":{"$exists":true}}, "validationLevel":"strict", "validationAction":"error"}).await.unwrap();
    let before = documents(&store).await;
    assert_eq!(
        store
            .admit_pre_exclusion(policy.clone(), input.clone())
            .await,
        Err(Domain::Unavailable)
    );
    assert_eq!(documents(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":records::AUDITS,"validator":{}})
        .await
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(
        transaction::apply(&store, &policy, &Change::Admit(input.clone()), &mut session)
            .await
            .is_ok()
    );
    session.abort_transaction().await.unwrap();
    assert_eq!(documents(&store).await, before);
    assert!(store.admit_pre_exclusion(policy, input).await.is_ok());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set with test commands"]
async fn unknown_commit_reconciles_original_admission_and_replay_is_zero_write() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    client.database("admin").run_command(doc! {"configureFailPoint":"failCommand", "mode":{"times":5},
        "data":{"failCommands":["commitTransaction"], "writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired", "errmsg":"injected ambiguous admission"}}}).await.unwrap();
    let outcome = store
        .admit_pre_exclusion(policy.clone(), input.clone())
        .await;
    client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let original = outcome.unwrap();
    let before = documents(&store).await;
    assert_eq!(before[6].len(), 1);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.admit_pre_exclusion(policy, input).await.unwrap(),
        original
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn source_lane_race_and_refreshed_source_refuse_without_partial_admission() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    ledger_maintenance_snapshot::load_pre_exclusion(
        &store,
        &input.caller,
        &input.accepted.plan().sandbox_id,
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
        matches!(transaction::apply(&store, &policy, &Change::Admit(input.clone()), &mut session).await, Err(ApplyError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(documents(&store).await, before);
    assert_eq!(
        store.admit_pre_exclusion(policy, input).await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn missing_audit_and_malformed_or_version_downgraded_marker_refuse() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    let state = store
        .admit_pre_exclusion(policy.clone(), input.clone())
        .await
        .unwrap();
    let held_parent = store.sandboxes.find_one(doc! {}).await.unwrap().unwrap();
    assert!(records::read(&held_parent).unwrap().is_some());
    transaction::audit_collection(&store)
        .delete_many(doc! {})
        .await
        .unwrap();
    let before = documents(&store).await;
    assert!(matches!(
        store
            .inspect_pre_exclusion(&input.caller, &input.accepted.plan().sandbox_id)
            .await,
        Err(Domain::Corrupt)
    ));
    assert_eq!(
        store
            .progress_pre_exclusion(policy, progress(&input, state))
            .await,
        Err(Domain::Corrupt)
    );
    assert_eq!(documents(&store).await, before);
    for fields in [
        doc! {records::FIELD:"malformed"},
        doc! {"document_schema_version":1_i64},
    ] {
        store
            .sandboxes
            .replace_one(doc! {}, held_parent.clone())
            .await
            .unwrap();
        store
            .sandboxes
            .update_one(doc! {}, doc! {"$set":fields})
            .await
            .unwrap();
        assert!(records::read(&store.sandboxes.find_one(doc! {}).await.unwrap().unwrap()).is_err());
        let before = documents(&store).await;
        assert!(matches!(
            store
                .inspect_pre_exclusion(&input.caller, &input.accepted.plan().sandbox_id)
                .await,
            Err(Domain::Corrupt)
        ));
        assert_eq!(documents(&store).await, before);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn neither_admission_port_can_reinterpret_the_other_format() {
    use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionPolicy;
    use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStore;
    for pre_first in [false, true] {
        let (client, name, store) = database().await;
        // The existing fixture builds an independently valid V1 request first.
        let v1 = support::fixture(&store, true).await;
        let old = v1.accepted.plan();
        let source = store
            .inspect_pre_exclusion(&v1.caller, &old.source.sandbox.id)
            .await
            .unwrap()
            .source;
        let plan = LedgerPreExclusionPlanV1 {
            format: "ledger-pre-exclusion-plan-v1".into(),
            operation_id: "distinct-fixture-operation".into(),
            sandbox_id: old.source.sandbox.id.clone(),
            home_identity: old.binding.home_identity.clone(),
            generation: old.binding.generation,
            boot_id: old.binding.boot_id.clone(),
            roots_sha256: old.binding.roots_sha256.clone(),
            source,
            disposition: LedgerRecoveryDispositionV1::PreserveHeld,
            authorized_by: old.source.sandbox.owner.clone(),
            consent_receipt_sha256: old.consent_receipt_sha256.clone(),
            writer_barrier_epoch: 1,
            compatible_floor: LedgerMaintenanceFloorV1 {
                reader_manifest_sha256: "a".repeat(64),
                writer_manifest_sha256: "b".repeat(64),
            },
            requested_at: NOW.into(),
        };
        let policy = Policy {
            enabled: true,
            qualified_floor: Some(plan.compatible_floor.clone()),
        };
        let pre = LedgerPreExclusionAdmission {
            caller: v1.caller.clone(),
            accepted: LedgerPreExclusionArtifactV1::parse(&serde_json::to_vec(&plan).unwrap())
                .unwrap(),
            lease: v1.lease.clone(),
            now: NOW.into(),
        };
        let enabled = LedgerMaintenanceAdmissionPolicy { enabled: true };
        if pre_first {
            store
                .admit_pre_exclusion(policy.clone(), pre.clone())
                .await
                .unwrap();
        } else {
            store
                .admit_ledger_maintenance(enabled, v1.clone())
                .await
                .unwrap();
        }
        let before = documents(&store).await;
        if pre_first {
            assert_eq!(
                store.admit_ledger_maintenance(enabled, v1).await,
                Err(Domain::Refused(Refusal::Conflict))
            );
        } else {
            assert_eq!(
                store.admit_pre_exclusion(policy, pre).await,
                Err(Domain::Refused(Refusal::Conflict))
            );
        }
        assert_eq!(documents(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn scoped_parent_version_never_accepts_version_two_in_other_collections() {
    let (client, name, store) = database().await;
    fixture(&store).await;
    store
        .workspaces
        .update_one(
            doc! {},
            doc! {"$set":{"document_schema_version":records::PARENT_VERSION}},
        )
        .await
        .unwrap();
    let before = documents(&store).await;
    assert!(MongoSandboxStore::open(&client, &name).await.is_err());
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}
