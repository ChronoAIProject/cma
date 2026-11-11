use super::super::ledger_maintenance_pre_exclusion_records as records;
use super::super::ledger_maintenance_test_support::{self as support, EXPIRED, NEXT, NOW};
use super::super::sandbox_maintenance::mongo_integration_tests::database;
use super::*;
use layer2_core::sandbox::agent_ledger_recovery::LedgerRecoveryDispositionV1;
use layer2_core::sandbox::ledger_maintenance_pre_exclusion::{
    LedgerMaintenanceFloorV1, LedgerPreExclusionArtifactV1, LedgerPreExclusionPhaseV1 as Phase,
    LedgerPreExclusionPlanV1,
};
use layer2_core::sandbox::ledger_maintenance_pre_exclusion_progress::LedgerPreExclusionAction as Action;
use layer2_core::sandbox::store::{SandboxStore, SandboxStoreError, SandboxWorkerStore};
use layer2_core::{Authorization, Capability, SubjectId};
use mongodb::bson::{doc, Document};

#[path = "ledger_maintenance_pre_exclusion_failure_tests.rs"]
mod failures;

#[path = "ledger_maintenance_pre_exclusion_cancellation_tests.rs"]
mod cancellation;

#[path = "ledger_maintenance_input_tests.rs"]
mod inputs;

async fn fixture(store: &MongoSandboxStore) -> (Policy, LedgerPreExclusionAdmission) {
    let prior = support::fixture(store, true).await;
    let old = prior.accepted.plan();
    let source = store
        .inspect_pre_exclusion(&prior.caller, &old.source.sandbox.id)
        .await
        .unwrap()
        .source;
    let plan = LedgerPreExclusionPlanV1 {
        format: "ledger-pre-exclusion-plan-v1".into(),
        operation_id: "pre-exclusion-fixture-operation".into(),
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
    let request = LedgerPreExclusionAdmission {
        caller: prior.caller,
        accepted: LedgerPreExclusionArtifactV1::parse(&serde_json::to_vec_pretty(&plan).unwrap())
            .unwrap(),
        lease: prior.lease,
        now: NOW.into(),
    };
    (policy, request)
}

async fn documents(store: &MongoSandboxStore) -> Vec<Vec<Document>> {
    use futures_util::TryStreamExt;
    let mut rows = support::documents(store).await;
    rows.push(
        transaction::audit_collection(store)
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap(),
    );
    rows
}

fn progress(input: &LedgerPreExclusionAdmission, state: State) -> LedgerPreExclusionProgress {
    LedgerPreExclusionProgress {
        caller: input.caller.clone(),
        expected: state,
        expected_source: input.accepted.plan().source.clone(),
        holder: input.lease.holder.clone(),
        now: NOW.into(),
        action: Action::BeginDrain,
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn independent_admissions_restart_and_later_takeover_replay_original_outcomes() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (a, b) = tokio::join!(
        store.admit_pre_exclusion(policy.clone(), input.clone()),
        other.admit_pre_exclusion(policy.clone(), input.clone())
    );
    let original = a.unwrap();
    assert_eq!(original, b.unwrap());
    assert_eq!(
        original.accepted_admission_bytes,
        input.accepted.original_bytes()
    );
    let mut change = progress(&input, original.clone());
    let draining = store
        .progress_pre_exclusion(policy.clone(), change.clone())
        .await
        .unwrap();
    assert_eq!(draining.phase, Phase::Draining);
    let first_progress = change.clone();
    change.expected = draining.clone();
    change.now = EXPIRED.into();
    change.action = Action::Takeover {
        lease: support::lease(NEXT),
    };
    let later = store
        .progress_pre_exclusion(policy.clone(), change)
        .await
        .unwrap();
    assert_eq!(later.writer_barrier_epoch, 2);
    assert_eq!(later.record_revision, 3);
    let before = documents(&store).await;
    assert_eq!(before[6].len(), 3);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .admit_pre_exclusion(policy.clone(), input.clone())
            .await
            .unwrap(),
        original
    );
    assert_eq!(
        reopened
            .progress_pre_exclusion(policy, first_progress)
            .await
            .unwrap(),
        draining
    );
    let inspection = reopened
        .inspect_pre_exclusion(&input.caller, &input.accepted.plan().sandbox_id)
        .await
        .unwrap();
    assert_eq!(inspection.state, Some(later));
    assert!(inspection.source == input.accepted.plan().source);
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn old_reader_refuses_but_current_read_preserves_source_and_parent_save_cannot_drop_hold() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    let source = &input.accepted.plan().source;
    let old = store.sandboxes.find_one(doc! {}).await.unwrap().unwrap();
    assert!(
        super::super::sandbox_mongo::read_record::<layer2_core::sandbox::Sandbox>(&old).is_ok()
    );
    store
        .admit_pre_exclusion(policy, input.clone())
        .await
        .unwrap();
    let held = store.sandboxes.find_one(doc! {}).await.unwrap().unwrap();
    assert_eq!(
        held.get_i64("document_schema_version").unwrap(),
        records::PARENT_VERSION
    );
    assert!(
        super::super::sandbox_mongo::read_record::<layer2_core::sandbox::Sandbox>(&held).is_err()
    );
    assert!(
        store
            .get_sandbox(&source.sandbox.id)
            .await
            .unwrap()
            .unwrap()
            == source.sandbox
    );
    let before = documents(&store).await;
    let mut next = source.sandbox.clone();
    next.revision += 1;
    assert_eq!(
        store.save_sandbox(source.sandbox.revision, next).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn disabled_missing_floor_foreign_and_changed_artifacts_write_nothing() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    let before = documents(&store).await;
    assert!(store
        .inspect_pre_exclusion(&input.caller, &input.accepted.plan().sandbox_id)
        .await
        .unwrap()
        .state
        .is_none());
    assert_eq!(
        store
            .admit_pre_exclusion(Policy::default(), input.clone())
            .await,
        Err(Domain::Refused(Refusal::Disabled))
    );
    assert_eq!(
        store
            .admit_pre_exclusion(
                Policy {
                    enabled: true,
                    qualified_floor: None
                },
                input.clone()
            )
            .await,
        Err(Domain::Refused(Refusal::InvalidEvidence))
    );
    for caller in [
        Caller::new(input.caller.subject.clone(), Authorization::none()),
        Caller::new(
            SubjectId("foreign-fixture".into()),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
    ] {
        let mut foreign = input.clone();
        foreign.caller = caller;
        assert_eq!(
            store.admit_pre_exclusion(policy.clone(), foreign).await,
            Err(Domain::Refused(Refusal::NotFound))
        );
    }
    assert_eq!(documents(&store).await, before);
    store
        .admit_pre_exclusion(policy.clone(), input.clone())
        .await
        .unwrap();
    let before = documents(&store).await;
    let mut changed = input.clone();
    changed.accepted =
        LedgerPreExclusionArtifactV1::parse(&serde_json::to_vec(input.accepted.plan()).unwrap())
            .unwrap();
    assert_eq!(
        store.admit_pre_exclusion(policy, changed).await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn competing_takeovers_and_abort_keep_hold_and_no_normal_writer_advances() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    let state = store
        .admit_pre_exclusion(policy.clone(), input.clone())
        .await
        .unwrap();
    let mut first = progress(&input, state);
    first.now = EXPIRED.into();
    first.action = Action::Takeover {
        lease: support::lease(NEXT),
    };
    let mut second = first.clone();
    second.holder = "other-fixture-worker".into();
    let mut other_lease = support::lease(NEXT);
    other_lease.holder = second.holder.clone();
    second.action = Action::Takeover { lease: other_lease };
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (a, b) = tokio::join!(
        store.progress_pre_exclusion(policy.clone(), first.clone()),
        other.progress_pre_exclusion(policy.clone(), second.clone())
    );
    let winner = match (a, b) {
        (Ok(state), Err(Domain::Refused(Refusal::Conflict)))
        | (Err(Domain::Refused(Refusal::Conflict)), Ok(state)) => state,
        outcomes => panic!("expected one epoch winner: {outcomes:?}"),
    };
    let mut abort = first;
    abort.holder = winner.lease.holder.clone();
    abort.expected = winner;
    abort.action = Action::AbortHeld;
    let aborted = store.progress_pre_exclusion(policy, abort).await.unwrap();
    assert_eq!(aborted.phase, Phase::AbortedHeld);
    let before = documents(&store).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(matches!(
        super::super::ledger_maintenance_writer::fence_writer(
            &store,
            &input.accepted.plan().sandbox_id,
            layer2_core::sandbox::ledger_maintenance::LedgerWriterClass::Purge,
            None,
            &mut session
        )
        .await,
        Err(ApplyError::Domain(Domain::WriterRefused(
            layer2_core::sandbox::ledger_maintenance::LedgerMaintenanceRefusal::Held
        )))
    ));
    session.abort_transaction().await.unwrap();
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn ordinary_parent_save_positive_control_still_succeeds_without_a_hold() {
    let (client, name, store) = database().await;
    let (_, input) = fixture(&store).await;
    let current = &input.accepted.plan().source.sandbox;
    let mut next = current.clone();
    next.revision += 1;
    store
        .save_sandbox(current.revision, next.clone())
        .await
        .unwrap();
    assert!(store.get_sandbox(&current.id).await.unwrap().unwrap() == next);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn int32_source_version_preserves_the_new_snapshot_through_admission_and_progress() {
    let (client, name, store) = database().await;
    let (policy, input) = fixture(&store).await;
    store
        .sandboxes
        .update_one(doc! {}, doc! {"$set":{"document_schema_version":1_i32}})
        .await
        .unwrap();
    let observed = store
        .inspect_pre_exclusion(&input.caller, &input.accepted.plan().sandbox_id)
        .await
        .unwrap();
    assert!(observed.source == input.accepted.plan().source);
    let admitted = store
        .admit_pre_exclusion(policy.clone(), input.clone())
        .await
        .unwrap();
    let draining = store
        .progress_pre_exclusion(policy, progress(&input, admitted))
        .await
        .unwrap();
    assert_eq!(draining.phase, Phase::Draining);
    assert!(
        store
            .inspect_pre_exclusion(&input.caller, &input.accepted.plan().sandbox_id)
            .await
            .unwrap()
            .source
            == input.accepted.plan().source
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn eligible_purge_control_succeeds_but_a_hold_survives_even_a_stale_retired_projection() {
    use layer2_core::sandbox::{SandboxStatus, SandboxWorkState, SandboxWorkStep};
    for held in [false, true] {
        let (client, name, store) = database().await;
        let (policy, input) = fixture(&store).await;
        if held {
            store
                .admit_pre_exclusion(policy, input.clone())
                .await
                .unwrap();
        }
        // Simulate an already-inflight older projection publishing Retired after
        // E1. Even that stale write cannot make a retained hold purgeable.
        let original = store.sandboxes.find_one(doc! {}).await.unwrap().unwrap();
        let mut retired = input.accepted.plan().source.sandbox.clone();
        retired.status = SandboxStatus::Retired;
        retired.runtime = None;
        retired.lifecycle.suspended_runtime = None;
        retired.lifecycle.home = None;
        retired.credential.current = None;
        retired.credential.delivered = false;
        retired.credential.revoke_debt.clear();
        retired.work = SandboxWorkState::new(SandboxWorkStep::Purge, NOW.into());
        retired.retired_at = Some(NOW.into());
        retired.purge_after = Some(NOW.into());
        let mut document = super::super::sandboxes::sandbox_document(&retired, 0).unwrap();
        if held {
            document.insert("document_schema_version", records::PARENT_VERSION);
            document.insert(
                records::FIELD,
                original.get(records::FIELD).unwrap().clone(),
            );
            document.insert("ledger_maintenance_fence_v1", 1_i64);
        }
        store
            .sandboxes
            .replace_one(doc! {}, document)
            .await
            .unwrap();
        let before = documents(&store).await;
        let outcome = store
            .purge_sandbox(&retired.id, retired.revision, NOW)
            .await;
        if held {
            assert_eq!(outcome, Err(SandboxStoreError::Conflict));
            assert_eq!(documents(&store).await, before);
        } else {
            outcome.unwrap();
            assert!(store.get_sandbox(&retired.id).await.unwrap().is_none());
        }
        client.database(&name).drop().await.unwrap();
    }
}
