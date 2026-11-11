use super::*;
use layer2_core::sandbox::ledger_maintenance::LedgerMaintenancePhaseV1 as Phase;
use layer2_core::sandbox::ledger_maintenance_progress::{
    LedgerMaintenanceEarlyAction as Action, LedgerMaintenanceProgress as Progress,
};

const PROGRESS_AT: &str = "2026-09-05T00:30:03Z";

#[path = "ledger_maintenance_cancellation_tests.rs"]
mod cancellation;

async fn admitted(store: &MongoSandboxStore) -> Progress {
    let request = fixture(store, true).await;
    let expected = store
        .admit_ledger_maintenance(ENABLED, request.clone())
        .await
        .unwrap();
    Progress {
        caller: request.caller,
        holder: expected.lease.holder.clone(),
        expected,
        expected_source: request.accepted.plan().source.clone(),
        now: PROGRESS_AT.into(),
        action: Action::BeginDrain,
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn independent_stores_progress_once_and_replay_original_outcome_after_takeover() {
    let (client, name, store) = database().await;
    let input = admitted(&store).await;
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (a, b) = tokio::join!(
        store.progress_ledger_maintenance(ENABLED, input.clone()),
        other.progress_ledger_maintenance(ENABLED, input.clone())
    );
    let draining = a.unwrap();
    assert_eq!(draining, b.unwrap());
    assert_eq!(draining.record.phase, Phase::Draining);
    assert_eq!(draining.record.record_revision, 2);
    assert!(draining.lease == input.expected.lease);
    assert_eq!(
        draining.accepted_plan_bytes,
        input.expected.accepted_plan_bytes
    );
    let rows = documents(&store).await;
    assert_eq!(rows[5].len(), 2);
    for table in rows.iter().take(5) {
        assert_eq!(table[0].get_i64("ledger_maintenance_fence_v1").unwrap(), 2);
    }
    let takeover = LedgerMaintenanceTakeover {
        caller: input.caller.clone(),
        expected: draining.clone(),
        expected_source: input.expected_source.clone(),
        lease: lease(NEXT),
        now: EXPIRED.into(),
    };
    let later = store
        .take_over_ledger_maintenance(ENABLED, takeover)
        .await
        .unwrap();
    assert_eq!(later.record.binding.maintenance_epoch, 2);
    assert_eq!(later.record.phase, Phase::Draining);
    let before = documents(&store).await;
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .progress_ledger_maintenance(ENABLED, input.clone())
            .await
            .unwrap(),
        draining
    );
    assert_eq!(documents(&store).await, before);
    let mut changed_request = input;
    changed_request.action = Action::AbortHeld;
    assert_eq!(
        reopened
            .progress_ledger_maintenance(ENABLED, changed_request)
            .await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn abort_retains_originals_and_hold_without_lifecycle_reset_or_late_takeover() {
    let (client, name, store) = database().await;
    let mut input = admitted(&store).await;
    input.expected = store
        .progress_ledger_maintenance(ENABLED, input.clone())
        .await
        .unwrap();
    input.action = Action::AbortHeld;
    let aborted = store
        .progress_ledger_maintenance(ENABLED, input.clone())
        .await
        .unwrap();
    assert_eq!(aborted.record.phase, Phase::AbortedHeld);
    assert_eq!(aborted.record.record_revision, 3);
    assert_eq!(
        aborted.accepted_plan_bytes,
        input.expected.accepted_plan_bytes
    );
    let inspected = store
        .inspect_ledger_maintenance(&input.caller, &input.expected_source.sandbox.id)
        .await
        .unwrap();
    assert!(inspected.source == input.expected_source);
    assert_eq!(inspected.state, Some(aborted.clone()));
    let before = documents(&store).await;
    assert_eq!(
        store
            .progress_ledger_maintenance(ENABLED, input.clone())
            .await
            .unwrap(),
        aborted
    );
    assert_eq!(
        store
            .take_over_ledger_maintenance(
                ENABLED,
                LedgerMaintenanceTakeover {
                    caller: input.caller.clone(),
                    expected: aborted,
                    expected_source: input.expected_source.clone(),
                    lease: lease(NEXT),
                    now: EXPIRED.into(),
                }
            )
            .await,
        Err(Domain::Refused(Refusal::StaleLease))
    );
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(matches!(
        crate::store::ledger_maintenance_writer::fence_writer(
            &store,
            &input.expected_source.sandbox.id,
            layer2_core::sandbox::ledger_maintenance::LedgerWriterClass::HomeCleanup,
            None,
            &mut session
        )
        .await,
        Err(ApplyError::Domain(Domain::WriterRefused(_)))
    ));
    session.abort_transaction().await.unwrap();
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn disabled_foreign_stale_holder_and_expired_commands_write_nothing() {
    let (client, name, store) = database().await;
    let input = admitted(&store).await;
    let before = documents(&store).await;
    assert_eq!(
        store
            .progress_ledger_maintenance(Policy::default(), input.clone())
            .await,
        Err(Domain::Refused(Refusal::Disabled))
    );
    for case in 0..4 {
        let mut bad = input.clone();
        let reason = match case {
            0 => {
                bad.caller = Caller::new(
                    SubjectId("foreign-fixture".into()),
                    Authorization::new([Capability::UseOwnSandboxes]),
                );
                Refusal::NotFound
            }
            1 => {
                bad.caller.authorization = Authorization::none();
                Refusal::NotFound
            }
            2 => {
                bad.holder = "stale-worker".into();
                Refusal::StaleLease
            }
            _ => {
                bad.now = EXPIRED.into();
                Refusal::StaleLease
            }
        };
        assert_eq!(
            store.progress_ledger_maintenance(ENABLED, bad).await,
            Err(Domain::Refused(reason)),
            "case {case}"
        );
        assert_eq!(documents(&store).await, before);
    }
    let mut operator = input;
    operator.caller = Caller::new(
        SubjectId("operator-fixture".into()),
        Authorization::new([Capability::ControlAnySandbox]),
    );
    let state = store
        .progress_ledger_maintenance(ENABLED, operator)
        .await
        .unwrap();
    let audit = transaction::audit_collection(&store)
        .find_one(doc! {"_id":records::state_hash(&state).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(audit.get_str("authority").unwrap(), "operator");
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn changed_source_and_missing_audit_refuse_instead_of_reopening_the_workflow() {
    let (client, name, store) = database().await;
    let input = admitted(&store).await;
    store
        .agents
        .update_one(doc! {}, doc! {"$set":{"fixture_sidecar":1_i64}})
        .await
        .unwrap();
    let before = documents(&store).await;
    assert_eq!(
        store
            .progress_ledger_maintenance(ENABLED, input.clone())
            .await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    assert_eq!(documents(&store).await, before);
    store
        .agents
        .update_one(doc! {}, doc! {"$unset":{"fixture_sidecar":""}})
        .await
        .unwrap();
    transaction::audit_collection(&store)
        .delete_many(doc! {})
        .await
        .unwrap();
    let before = documents(&store).await;
    assert_eq!(
        store.progress_ledger_maintenance(ENABLED, input).await,
        Err(Domain::Corrupt)
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn audit_failure_and_explicit_abort_rollback_progress_and_every_fence() {
    let (client, name, store) = database().await;
    let input = admitted(&store).await;
    client.database(&name).run_command(doc! {"collMod":records::AUDITS,
        "validator":{"revision":{"$lte":1_i64}}, "validationLevel":"strict", "validationAction":"error"}).await.unwrap();
    let before = documents(&store).await;
    assert_eq!(
        store
            .progress_ledger_maintenance(ENABLED, input.clone())
            .await,
        Err(Domain::Unavailable)
    );
    assert_eq!(documents(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":records::AUDITS, "validator":{}})
        .await
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(transaction::apply(
        &store,
        &Change::Progress(ENABLED, input.clone()),
        &mut session
    )
    .await
    .is_ok());
    session.abort_transaction().await.unwrap();
    assert_eq!(documents(&store).await, before);
    assert_eq!(
        store
            .progress_ledger_maintenance(ENABLED, input)
            .await
            .unwrap()
            .record
            .phase,
        Phase::Draining
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set with test commands"]
async fn ambiguous_progress_commit_reconciles_exactly_once_then_replays_without_writes() {
    let (client, name, store) = database().await;
    let input = admitted(&store).await;
    client
        .database("admin")
        .run_command(
            doc! {"configureFailPoint":"failCommand", "mode":{"times":5},
            "data":{"failCommands":["commitTransaction"], "writeConcernError":{
                "code":50, "codeName":"MaxTimeMSExpired", "errmsg":"injected ambiguous progress"}}},
        )
        .await
        .unwrap();
    let outcome = store
        .progress_ledger_maintenance(ENABLED, input.clone())
        .await;
    client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    let state = outcome.unwrap();
    assert_eq!(state.record.phase, Phase::Draining);
    let before = documents(&store).await;
    assert_eq!(before[5].len(), 2);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .progress_ledger_maintenance(ENABLED, input)
            .await
            .unwrap(),
        state
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn lane_race_conflicts_before_progress_and_cannot_be_accepted_by_refreshing_source() {
    let (client, name, store) = database().await;
    let mut input = admitted(&store).await;
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    super::super::super::ledger_maintenance_snapshot::load(
        &store,
        &input.caller,
        &input.expected_source.sandbox.id,
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
        matches!(transaction::apply(&store, &Change::Progress(ENABLED, input.clone()), &mut session).await,
        Err(ApplyError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(documents(&store).await, before);
    input.expected_source = other
        .inspect_ledger_maintenance(&input.caller, &input.expected_source.sandbox.id)
        .await
        .unwrap()
        .source;
    assert_eq!(
        other.progress_ledger_maintenance(ENABLED, input).await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}
