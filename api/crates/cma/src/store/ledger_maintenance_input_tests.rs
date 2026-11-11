//! Real owner/command ports with the proposed A transactional hooks applied.
use super::*;
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::store::{
    AgentControlResult, AgentControlStore, AgentControlStoreError, AgentControlTarget,
    AgentDeleteRequest, AgentStopRequest, AgentStore,
};
use layer2_core::sandbox::{AgentStatus, AgentStopReason};

#[path = "ledger_maintenance_input_test_support.rs"]
mod helpers;
use helpers::*;

#[path = "ledger_maintenance_command_tests.rs"]
mod command_mutations;

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and shared input hooks"]
async fn new_input_refuses_after_hold_restart_and_lease_expiry_without_any_writes() {
    for held in [false, true] {
        let (client, name, store) = database().await;
        let (policy, input) = fixture(&store).await;
        let source = &input.accepted.plan().source;
        let request = reservation(&source.agents[0], &source.sandbox, "fresh-input");
        if held {
            store
                .admit_pre_exclusion(policy, input.clone())
                .await
                .unwrap();
        }
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let before = all_rows(&store).await;
        if held {
            for at in [NOW, EXPIRED] {
                let mut request = request.clone();
                request.admitted_at = at.into();
                assert_eq!(
                    reopened.reserve_command(request).await,
                    Err(CommandLedgerError::NotAdmissible)
                );
                assert_eq!(all_rows(&store).await, before);
            }
        } else {
            assert!(matches!(
                reopened.reserve_command(request).await.unwrap(),
                CommandReservationResult::Created(_)
            ));
            assert_eq!(
                store.agent_commands.count_documents(doc! {}).await.unwrap(),
                1
            );
            assert_ne!(all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and shared input hooks"]
async fn historical_input_replays_exactly_under_hold_but_cannot_acquire_a_new_command_claim() {
    let (client, name, store) = database().await;
    let (policy, mut input) = fixture(&store).await;
    let source = &input.accepted.plan().source;
    let agent = source.agents[0].clone();
    let request = reservation(&agent, &source.sandbox, "original-input");
    let original = match store.reserve_command(request.clone()).await.unwrap() {
        CommandReservationResult::Created(record) => record,
        _ => panic!("fixture input was not newly reserved"),
    };
    refresh(&store, &mut input).await;
    store.admit_pre_exclusion(policy, input).await.unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let before = all_rows(&store).await;
    assert_eq!(
        reopened.reserve_command(request).await.unwrap(),
        CommandReservationResult::Existing(original.clone())
    );
    for at in [NOW, EXPIRED] {
        assert_eq!(
            reopened.claim_next_command(claim_request(&agent, at)).await,
            Err(CommandLedgerError::NotAdmissible)
        );
        assert_eq!(
            reopened
                .claim_observed_command(
                    claim_request(&agent, at),
                    &original.receipt.envelope.command_id,
                    original.revision
                )
                .await,
            Err(CommandLedgerError::NotAdmissible)
        );
    }
    assert_eq!(all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and shared input hooks"]
async fn real_command_claim_succeeds_without_hold_and_invalidates_a_proposed_admission() {
    let (client, name, store) = database().await;
    let (policy, mut input) = fixture(&store).await;
    let source = &input.accepted.plan().source;
    let agent = source.agents[0].clone();
    store
        .reserve_command(reservation(&agent, &source.sandbox, "queued-input"))
        .await
        .unwrap();
    refresh(&store, &mut input).await;
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let claimed = other
        .claim_next_command(claim_request(&agent, NOW))
        .await
        .unwrap()
        .unwrap();
    assert!(claimed.work.lease.is_some());
    let before = all_rows(&store).await;
    assert_eq!(
        store.admit_pre_exclusion(policy, input).await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    assert_eq!(all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and shared input hooks"]
async fn input_commit_conflicts_with_an_inflight_maintenance_snapshot_and_rolls_back() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    let source = &input.accepted.plan().source;
    let request = reservation(&source.agents[0], &source.sandbox, "racing-input");
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    ledger_maintenance_snapshot::load_pre_exclusion(
        &store,
        &input.caller,
        &source.sandbox.id,
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("fixture snapshot failed"));
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(matches!(
        other.reserve_command(request).await.unwrap(),
        CommandReservationResult::Created(_)
    ));
    let before = all_rows(&store).await;
    assert!(
        matches!(transaction::apply(&store, &policy, &Change::Admit(input.clone()), &mut session).await, Err(ApplyError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(all_rows(&store).await, before);
    assert_eq!(
        store.admit_pre_exclusion(policy, input).await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    assert_eq!(all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and shared input hooks"]
async fn owner_stop_and_delete_have_positive_controls_and_hold_refusal_rolls_back_all_lineage() {
    for delete in [false, true] {
        for held in [false, true] {
            let (client, name, store) = database().await;
            let (policy, input) = control_fixture(&store, delete).await;
            let agent = input.accepted.plan().source.agents[0].clone();
            if held {
                store.admit_pre_exclusion(policy, input).await.unwrap();
            }
            let other = MongoSandboxStore::open(&client, &name).await.unwrap();
            let before = all_rows(&store).await;
            let result = control(&other, &agent, delete).await;
            if held {
                assert_eq!(result, Err(AgentControlStoreError::Conflict));
                assert_eq!(all_rows(&store).await, before);
            } else {
                assert!(matches!(result.unwrap(), AgentControlResult::Scheduled(_)));
                assert_ne!(all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and shared input hooks"]
async fn historical_stop_and_delete_replay_without_new_lineage_writes_after_a_hold() {
    for delete in [false, true] {
        let (client, name, store) = database().await;
        let (policy, mut input) = control_fixture(&store, delete).await;
        let agent = input.accepted.plan().source.agents[0].clone();
        let scheduled = match control(&store, &agent, delete).await.unwrap() {
            AgentControlResult::Scheduled(agent) => agent,
            _ => panic!("fixture control was not newly scheduled"),
        };
        refresh(&store, &mut input).await;
        store.admit_pre_exclusion(policy, input).await.unwrap();
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let before = all_rows(&store).await;
        assert_eq!(
            control(&reopened, &agent, delete).await.unwrap(),
            AgentControlResult::Existing(scheduled)
        );
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and shared input hooks"]
async fn malformed_or_missing_hold_audit_refuses_input_claim_and_control_without_writes() {
    for malformed in [false, true] {
        let (client, name, store) = database().await;
        let (policy, mut input) = fixture(&store).await;
        let source = &input.accepted.plan().source;
        let agent = source.agents[0].clone();
        let request = reservation(&agent, &source.sandbox, "new-input");
        store
            .reserve_command(reservation(&agent, &source.sandbox, "queued-input"))
            .await
            .unwrap();
        refresh(&store, &mut input).await;
        store.admit_pre_exclusion(policy, input).await.unwrap();
        if malformed {
            store
                .sandboxes
                .update_one(doc! {}, doc! {"$set":{records::FIELD:"invalid-fixture"}})
                .await
                .unwrap();
        } else {
            transaction::audit_collection(&store)
                .delete_many(doc! {})
                .await
                .unwrap();
        }
        let before = all_rows(&store).await;
        assert_eq!(
            store.reserve_command(request).await,
            Err(CommandLedgerError::Corrupt)
        );
        assert_eq!(
            store.claim_next_command(claim_request(&agent, NOW)).await,
            Err(CommandLedgerError::Corrupt)
        );
        assert!(matches!(
            control(&store, &agent, false).await,
            Err(AgentControlStoreError::Corrupt(_))
        ));
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and shared input hooks"]
async fn v1_hold_also_refuses_real_ports_without_relying_on_the_v2_envelope_filter() {
    use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionPolicy;
    use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStore;
    let (client, name, store) = database().await;
    let mut input = support::fixture(&store, true).await;
    let source = &input.accepted.plan().source;
    let agent = source.agents[0].clone();
    let request = reservation(&agent, &source.sandbox, "new-v1-input");
    store
        .reserve_command(reservation(&agent, &source.sandbox, "queued-v1-input"))
        .await
        .unwrap();
    let mut plan = input.accepted.plan().clone();
    plan.source = store
        .inspect_ledger_maintenance(&input.caller, &plan.source.sandbox.id)
        .await
        .unwrap()
        .source;
    input.accepted = support::artifact(&plan);
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
    let before = all_rows(&store).await;
    assert_eq!(
        store.reserve_command(request).await,
        Err(CommandLedgerError::NotAdmissible)
    );
    assert_eq!(
        store.claim_next_command(claim_request(&agent, NOW)).await,
        Err(CommandLedgerError::NotAdmissible)
    );
    assert_eq!(
        control(&store, &agent, false).await,
        Err(AgentControlStoreError::Conflict)
    );
    assert_eq!(all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and shared input hooks"]
async fn hold_commit_conflicts_with_an_inflight_control_lineage_write() {
    use crate::store::agent_control_lineage::{
        fence_control_lineage, load_control_snapshot, LineageError,
    };
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    let target = AgentControlTarget::from(&input.accepted.plan().source.agents[0]);
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let snapshot = load_control_snapshot(&store, &target, &mut session, false)
        .await
        .unwrap_or_else(|_| panic!("fixture control snapshot failed"));
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    other.admit_pre_exclusion(policy, input).await.unwrap();
    let before = all_rows(&store).await;
    assert!(
        matches!(fence_control_lineage(&store, &snapshot, "fixture-control-operation", &mut session).await, Err(LineageError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and shared input hooks"]
async fn failed_command_insert_rolls_back_the_parent_admission_fence_and_lane() {
    let (client, name, store) = database().await;
    let (_, input) = fixture(&store).await;
    let source = &input.accepted.plan().source;
    let request = reservation(&source.agents[0], &source.sandbox, "failed-input");
    client.database(&name).run_command(doc! {"collMod":store.agent_commands.name(), "validator":{"reject_fixture":{"$exists":true}}, "validationLevel":"strict", "validationAction":"error"}).await.unwrap();
    let before = all_rows(&store).await;
    assert_eq!(
        store.reserve_command(request.clone()).await,
        Err(CommandLedgerError::Unavailable)
    );
    assert_eq!(all_rows(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":store.agent_commands.name(), "validator":{}})
        .await
        .unwrap();
    assert!(matches!(
        store.reserve_command(request).await.unwrap(),
        CommandReservationResult::Created(_)
    ));
    client.database(&name).drop().await.unwrap();
}
