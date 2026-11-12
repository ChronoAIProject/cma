//! Persistence fixtures only; these hashes cannot construct execution authority.
use crate::store::ledger_maintenance_test_support as support;
use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionPolicy;
use layer2_core::sandbox::ledger_maintenance_plan::LedgerMaintenancePlanV1;
use layer2_core::sandbox::store::ledger_maintenance::{
    LedgerMaintenanceAdmission, LedgerMaintenanceStore,
};
use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;
use layer2_core::sandbox::Sandbox;
use layer3_adapter::runtime_ledger_maintenance::{
    LedgerBindingV1, LedgerSourceFormatV1, LEDGER_BINDING_FORMAT,
};

pub(in crate::store) async fn hold(store: &MongoSandboxStore, parent: &Sandbox, v1: bool) {
    let (policy, input) = support::pre_exclusion(store, parent, support::NOW).await;
    if !v1 {
        store.admit_pre_exclusion(policy, input).await.unwrap();
        return;
    }
    // V1's digest has a separate domain from the earlier E1 observation.
    let source = store
        .inspect_ledger_maintenance(&input.caller, &parent.id)
        .await
        .unwrap()
        .source;
    let p = input.accepted.plan();
    let plan = LedgerMaintenancePlanV1 {
        format: "ledger-maintenance-plan-v1".into(),
        binding: LedgerBindingV1 {
            format: LEDGER_BINDING_FORMAT.into(),
            sandbox_id: parent.id.to_string(),
            home_identity: p.home_identity.clone(),
            generation: p.generation,
            boot_id: p.boot_id.clone(),
            roots_sha256: p.roots_sha256.clone(),
            source_format: LedgerSourceFormatV1::V1,
            archive_id: uuid::Uuid::from_u128(1).to_string(),
            maintenance_id: p.operation_id.clone(),
            maintenance_epoch: 1,
            sandbox_revision: source.sandbox.revision,
            candidate_sha256: "c".repeat(64),
            recovery_sha256: "d".repeat(64),
            exclusion_receipt_sha256: "e".repeat(64),
        },
        source,
        consent_receipt_sha256: p.consent_receipt_sha256.clone(),
        requested_at: input.now.clone(),
    };
    store
        .admit_ledger_maintenance(
            LedgerMaintenanceAdmissionPolicy { enabled: true },
            LedgerMaintenanceAdmission {
                caller: input.caller,
                accepted: support::artifact(&plan),
                lease: input.lease,
                now: input.now,
            },
        )
        .await
        .unwrap();
}

pub(in crate::store) async fn fence_value(store: &MongoSandboxStore, parent: &Sandbox) -> i64 {
    let row = store
        .sandboxes
        .find_one(mongodb::bson::doc! {"_id": parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    row.get("ledger_writer_admission_fence_v1")
        .map(|value| value.as_i64().expect("parent fence must be an integer"))
        .unwrap_or(0)
}
