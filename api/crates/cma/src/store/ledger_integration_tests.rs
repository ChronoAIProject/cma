//! Combined B/C persistence boundary. Scheduling state must not bypass a held
//! pre-exclusion envelope or replace its accepted original bytes.
use super::ledger_maintenance_test_support::{self as support, NOW};
use super::sandbox_maintenance::mongo_integration_tests::database;
use super::sandbox_mongo::MongoSandboxStore;
use layer2_core::sandbox::agent_ledger_recovery::LedgerRecoveryDispositionV1;
use layer2_core::sandbox::ledger_maintenance_pre_exclusion::{
    LedgerMaintenanceFloorV1, LedgerPreExclusionArtifactV1, LedgerPreExclusionPhaseV1,
    LedgerPreExclusionPlanV1,
};
use layer2_core::sandbox::ledger_maintenance_pre_exclusion_progress::{
    LedgerPreExclusionAction, LedgerPreExclusionAdmission, LedgerPreExclusionPolicy,
    LedgerPreExclusionProgress,
};
use layer2_core::sandbox::store::ledger_collection::checkpoint::{
    LedgerCollectionCheckpointMutation, LedgerCollectionCheckpointStore,
    LedgerCollectionCheckpointV2,
};
use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;
use mongodb::bson::doc;

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set"]
async fn collector_checkpoint_survives_pre_exclusion_and_cannot_bypass_its_writer_floor() {
    let (client, name, store) = database().await;
    let prior = support::fixture(&store, true).await;
    let previous = prior.accepted.plan();
    let parent = &previous.source.sandbox;
    let checkpoint = LedgerCollectionCheckpointV2::new(
        previous.binding.binding_sha256().unwrap(),
        1,
        7,
        "a".repeat(64),
        10,
        20,
    )
    .unwrap();
    store
        .retain_ledger_collection_checkpoint(&parent.id, parent.revision, checkpoint.clone())
        .await
        .unwrap();
    let source = store
        .inspect_pre_exclusion(&prior.caller, &parent.id)
        .await
        .unwrap()
        .source;
    let plan = LedgerPreExclusionPlanV1 {
        format: "ledger-pre-exclusion-plan-v1".into(),
        operation_id: "combined-pre-exclusion-fixture".into(),
        sandbox_id: parent.id.clone(),
        home_identity: previous.binding.home_identity.clone(),
        generation: previous.binding.generation,
        boot_id: previous.binding.boot_id.clone(),
        roots_sha256: previous.binding.roots_sha256.clone(),
        source,
        disposition: LedgerRecoveryDispositionV1::PreserveHeld,
        authorized_by: parent.owner.clone(),
        consent_receipt_sha256: previous.consent_receipt_sha256.clone(),
        writer_barrier_epoch: 1,
        compatible_floor: LedgerMaintenanceFloorV1 {
            reader_manifest_sha256: "b".repeat(64),
            writer_manifest_sha256: "c".repeat(64),
        },
        requested_at: NOW.into(),
    };
    let policy = LedgerPreExclusionPolicy {
        enabled: true,
        qualified_floor: Some(plan.compatible_floor.clone()),
    };
    let input = LedgerPreExclusionAdmission {
        caller: prior.caller.clone(),
        accepted: LedgerPreExclusionArtifactV1::parse(&serde_json::to_vec_pretty(&plan).unwrap())
            .unwrap(),
        lease: prior.lease.clone(),
        now: NOW.into(),
    };
    let state = store
        .admit_pre_exclusion(policy.clone(), input.clone())
        .await
        .unwrap();
    let before = store.sandboxes.find_one(doc! {}).await.unwrap().unwrap();
    assert_eq!(before.get_i64("document_schema_version").unwrap(), 2);
    assert!(store
        .update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            checkpoint.progress.revision,
            LedgerCollectionCheckpointMutation::Claim {
                holder_sha256: "d".repeat(64),
                now: 20,
                expires_at: 30,
            },
        )
        .await
        .is_err());
    assert!(store
        .retain_ledger_collection_checkpoint(&parent.id, parent.revision, checkpoint.clone())
        .await
        .is_err());
    assert_eq!(
        store.sandboxes.find_one(doc! {}).await.unwrap().unwrap(),
        before
    );
    let draining = store
        .progress_pre_exclusion(
            policy,
            LedgerPreExclusionProgress {
                caller: input.caller.clone(),
                expected: state,
                expected_source: plan.source,
                holder: input.lease.holder,
                now: NOW.into(),
                action: LedgerPreExclusionAction::BeginDrain,
            },
        )
        .await
        .unwrap();
    assert_eq!(draining.phase, LedgerPreExclusionPhaseV1::Draining);
    let after = store.sandboxes.find_one(doc! {}).await.unwrap().unwrap();
    assert_eq!(
        before.get("ledger_collection_v1"),
        after.get("ledger_collection_v1")
    );
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .ledger_collection_checkpoint(&parent.id)
            .await
            .unwrap(),
        Some(checkpoint)
    );
    let inspection = reopened
        .inspect_pre_exclusion(&input.caller, &parent.id)
        .await
        .unwrap();
    let actual = inspection.state.unwrap();
    assert_eq!(actual, draining);
    assert_eq!(
        actual.accepted_admission_bytes,
        input.accepted.original_bytes()
    );
    client.database(&name).drop().await.unwrap();
}
