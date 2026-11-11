use super::super::ledger_maintenance_records::{self as records, FIELD};
use super::super::ledger_maintenance_test_support::*;
use super::super::sandbox_maintenance::mongo_integration_tests::database;
use super::*;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceTakeover;
use layer2_core::{Authorization, Capability, SubjectId};
use mongodb::bson::{doc, Bson};

const ENABLED: Policy = Policy { enabled: true };

#[path = "ledger_maintenance_failure_tests.rs"]
mod failures;

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn inspect_and_disabled_admission_write_nothing_and_conceal_foreign_markers() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    let before = documents(&store).await;
    let id = &request.accepted.plan().source.sandbox.id;
    let inspection = store
        .inspect_ledger_maintenance(&request.caller, id)
        .await
        .unwrap();
    assert!(inspection.source == request.accepted.plan().source);
    assert!(inspection.state.is_none());
    assert_eq!(
        store
            .admit_ledger_maintenance(Policy::default(), request.clone())
            .await,
        Err(Domain::Refused(Refusal::Disabled))
    );
    assert_eq!(documents(&store).await, before);
    store
        .sandboxes
        .update_one(doc! {"_id":id.as_str()}, doc! {"$set":{FIELD:"malformed"}})
        .await
        .unwrap();
    let after_marker = documents(&store).await;
    let foreign = Caller::new(
        SubjectId("other-fixture-principal".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    assert!(matches!(
        store.inspect_ledger_maintenance(&foreign, id).await,
        Err(Domain::Refused(Refusal::NotFound))
    ));
    assert!(matches!(
        store.inspect_ledger_maintenance(&request.caller, id).await,
        Err(Domain::Corrupt)
    ));
    assert_eq!(documents(&store).await, after_marker);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn two_independent_stores_admit_once_and_reopen_replays_exact_bytes() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (a, b) = tokio::join!(
        store.admit_ledger_maintenance(ENABLED, request.clone()),
        other.admit_ledger_maintenance(ENABLED, request.clone())
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(a.accepted_plan_bytes, request.accepted.original_bytes());
    assert_eq!(
        transaction::audit_collection(&store)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let rows = documents(&store).await;
    for collection in rows.iter().take(5) {
        assert_eq!(
            collection[0]
                .get_i64("ledger_maintenance_fence_v1")
                .unwrap(),
            1
        );
    }
    let marker = rows[0][0].get_document(FIELD).unwrap();
    assert!(matches!(
        marker.get("accepted_plan_bytes"),
        Some(Bson::Binary(_))
    ));
    let mut later = request.clone();
    later.now = EXPIRED.into();
    later.lease = lease(NEXT);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .admit_ledger_maintenance(ENABLED, later)
            .await
            .unwrap(),
        a
    );
    assert_eq!(documents(&store).await, rows);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn competing_operations_refuse_and_absent_lanes_are_not_synthesized() {
    let (client, name, store) = database().await;
    let request = fixture(&store, false).await;
    let mut different = request.clone();
    let mut plan = different.accepted.plan().clone();
    plan.binding.maintenance_id = "other-operation".into();
    different.accepted = artifact(&plan);
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (a, b) = tokio::join!(
        store.admit_ledger_maintenance(ENABLED, request),
        other.admit_ledger_maintenance(ENABLED, different)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        a.err().or(b.err()),
        Some(Domain::Refused(Refusal::Conflict))
    ));
    assert_eq!(
        store
            .agent_command_orders
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
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
async fn lane_drift_and_missing_child_refuse_without_partial_fences() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    store
        .agent_command_orders
        .update_one(doc! {}, doc! {"$inc":{"next_fence":1_i64}})
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
        .agent_command_orders
        .update_one(doc! {}, doc! {"$inc":{"next_fence":-1_i64}})
        .await
        .unwrap();
    store.worktrees.delete_one(doc! {}).await.unwrap();
    let before = documents(&store).await;
    assert_eq!(
        store.admit_ledger_maintenance(ENABLED, request).await,
        Err(Domain::Corrupt)
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn audit_insert_failure_rolls_back_parent_and_every_child_fence() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    client
        .database(&name)
        .run_command(doc! {
            "collMod":records::AUDITS, "validator":{"impossible_fixture_field":{"$exists":true}},
            "validationLevel":"strict", "validationAction":"error",
        })
        .await
        .unwrap();
    let before = documents(&store).await;
    assert_eq!(
        store
            .admit_ledger_maintenance(ENABLED, request.clone())
            .await,
        Err(Domain::Unavailable)
    );
    assert_eq!(documents(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":records::AUDITS, "validator":{}})
        .await
        .unwrap();
    assert!(store
        .admit_ledger_maintenance(ENABLED, request)
        .await
        .is_ok());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn explicit_abort_before_commit_rolls_back_all_fences_and_audit() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    let before = documents(&store).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(transaction::apply(
        &store,
        &Change::Admit(ENABLED, request.clone()),
        &mut session
    )
    .await
    .is_ok());
    session.abort_transaction().await.unwrap();
    assert_eq!(documents(&store).await, before);
    assert!(store
        .admit_ledger_maintenance(ENABLED, request)
        .await
        .is_ok());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn takeover_preserves_original_request_and_replays_after_a_later_epoch() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    let first = store
        .admit_ledger_maintenance(ENABLED, request.clone())
        .await
        .unwrap();
    let mut takeover = LedgerMaintenanceTakeover {
        caller: request.caller.clone(),
        expected: first.clone(),
        expected_source: request.accepted.plan().source.clone(),
        lease: lease(NEXT),
        now: NOW.into(),
    };
    assert_eq!(
        store
            .take_over_ledger_maintenance(ENABLED, takeover.clone())
            .await,
        Err(Domain::Refused(Refusal::StaleLease))
    );
    takeover.now = EXPIRED.into();
    let second = store
        .take_over_ledger_maintenance(ENABLED, takeover.clone())
        .await
        .unwrap();
    assert_eq!(second.record.binding.maintenance_epoch, 2);
    assert_eq!(second.accepted_plan_bytes, first.accepted_plan_bytes);
    let third = LedgerMaintenanceTakeover {
        expected: second.clone(),
        now: NEXT.into(),
        lease: lease("2026-09-05T00:33:02Z"),
        ..takeover.clone()
    };
    let third = store
        .take_over_ledger_maintenance(ENABLED, third)
        .await
        .unwrap();
    assert_eq!(third.record.binding.maintenance_epoch, 3);
    let before = documents(&store).await;
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .take_over_ledger_maintenance(ENABLED, takeover)
            .await
            .unwrap(),
        second
    );
    assert_eq!(documents(&store).await, before);
    assert_eq!(
        transaction::audit_collection(&store)
            .count_documents(doc! {})
            .await
            .unwrap(),
        3
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn lost_commit_reply_is_reconciled_from_immutable_audit() {
    let (client, name, store) = database().await;
    let request = fixture(&store, true).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let receipt = transaction::apply(
        &store,
        &Change::Admit(ENABLED, request.clone()),
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("fixture admission failed"));
    let audit = receipt.audit.unwrap();
    // The database commits; the caller discards the response and reopens.
    session.commit_transaction().await.unwrap();
    let time = session.operation_time().unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(transaction::reconcile(&reopened, &audit, time)
        .await
        .unwrap());
    assert_eq!(
        reopened
            .admit_ledger_maintenance(ENABLED, request)
            .await
            .unwrap(),
        receipt.state
    );
    let mut changed = audit.clone();
    changed.insert("plan_sha256", "0".repeat(64));
    assert!(!transaction::reconcile(&reopened, &changed, time)
        .await
        .unwrap());
    let mut absent = audit;
    absent.insert("_id", "0".repeat(64));
    assert!(!transaction::reconcile(&reopened, &absent, time)
        .await
        .unwrap());
    client.database(&name).drop().await.unwrap();
}
