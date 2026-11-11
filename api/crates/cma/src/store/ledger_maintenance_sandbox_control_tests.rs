//! Registered under the existing real Sandbox-control service fixtures by A.
use super::sandbox_controls_tests::{context, Custody, Fixture, Membership};
use super::*;
use crate::store::ledger_maintenance_test_support as support;
use layer2_core::resource_access::*;
use layer2_core::sandbox::control::*;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionPolicy;
use layer2_core::sandbox::ledger_maintenance_plan::LedgerMaintenancePlanV1;
use layer2_core::sandbox::store::ledger_maintenance::{
    LedgerMaintenanceAdmission, LedgerMaintenanceStore,
};
use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;
use layer3_adapter::runtime_ledger_maintenance::{
    LedgerBindingV1, LedgerSourceFormatV1, LEDGER_BINDING_FORMAT,
};
use std::sync::Arc;

async fn retained_fixture(client: &Client, name: &str) -> Fixture {
    let store = Arc::new(MongoSandboxStore::open(client, name).await.unwrap());
    let input = support::fixture(&store, true).await;
    let parent = input.accepted.plan().source.sandbox.clone();
    let agent = input.accepted.plan().source.agents[0].clone();
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(Membership(MAX_MEMBERSHIP_AGE)),
    ));
    access
        .change_share(
            &context(parent.owner.as_str()),
            Some("fixture"),
            ShareCommand {
                selector: ResourceSelector::Sandbox {
                    sandbox_id: parent.id.clone(),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::OrgRoles,
                },
                expected_share_revision: None,
                idempotency_key: "share-maintenance-control".into(),
            },
        )
        .await
        .unwrap();
    Fixture {
        service: SandboxControlService::new(store.clone(), access, Arc::new(Custody)),
        store,
        parent,
        agent,
    }
}

async fn v1_request(f: &Fixture) -> LedgerMaintenanceAdmission {
    let now = layer2_core::now_rfc3339();
    let (_, input) = support::pre_exclusion(&f.store, &f.parent, &now).await;
    // The V1 admission digest has its own domain; do not substitute E1 bytes.
    let source = f
        .store
        .inspect_ledger_maintenance(&input.caller, &f.parent.id)
        .await
        .unwrap()
        .source;
    let p = input.accepted.plan();
    // Disposable persistence evidence only; no physical exclusion authority.
    let plan = LedgerMaintenancePlanV1 {
        format: "ledger-maintenance-plan-v1".into(),
        binding: LedgerBindingV1 {
            format: LEDGER_BINDING_FORMAT.into(),
            sandbox_id: source.sandbox.id.to_string(),
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
        requested_at: now.clone(),
    };
    LedgerMaintenanceAdmission {
        caller: input.caller,
        accepted: support::artifact(&plan),
        lease: input.lease,
        now,
    }
}

async fn hold(f: &Fixture, v1: bool) {
    if v1 {
        f.store
            .admit_ledger_maintenance(
                LedgerMaintenanceAdmissionPolicy { enabled: true },
                v1_request(f).await,
            )
            .await
            .unwrap();
    } else {
        let (policy, input) =
            support::pre_exclusion(&f.store, &f.parent, &layer2_core::now_rfc3339()).await;
        f.store.admit_pre_exclusion(policy, input).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and control hooks"]
async fn controls_have_unheld_admission_replay_apply_and_reject_positive_controls() {
    for reject in [false, true] {
        let (client, name) = database("maintenance_controls_positive").await;
        let f = retained_fixture(&client, &name).await;
        let record = f.admit("original").await.unwrap();
        let before = support::all_rows(&f.store).await;
        assert_eq!(f.admit("original").await.unwrap(), record);
        assert_ne!(support::all_rows(&f.store).await, before);
        let result = if reject {
            f.store
                .reject_sandbox_control(&record, SandboxControlFailure::AuthorityRevoked)
                .await
                .unwrap()
        } else {
            f.store
                .apply_sandbox_control(&record, f.proof(&record, MAX_MEMBERSHIP_AGE).await)
                .await
                .unwrap()
        };
        assert_eq!(result.admission_audit, record.admission_audit);
        if reject {
            assert!(matches!(
                result.status,
                SandboxControlStatus::Rejected { .. }
            ));
            assert_eq!(
                f.store.get_sandbox(&f.parent.id).await.unwrap(),
                Some(f.parent)
            );
        } else {
            assert!(matches!(
                result.status,
                SandboxControlStatus::Applied { .. }
            ));
            assert!(result.execution_audit.is_some());
        }
        assert_eq!(
            f.store
                .sandbox_control(&record.envelope.command_id)
                .await
                .unwrap(),
            Some(result)
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and control hooks"]
async fn both_holds_refuse_new_replayed_applied_and_rejected_controls_after_reopen() {
    for v1 in [false, true] {
        let (client, name) = database("maintenance_controls_held").await;
        let f = retained_fixture(&client, &name).await;
        let record = f.admit("original").await.unwrap();
        let proof = f.proof(&record, MAX_MEMBERSHIP_AGE).await;
        hold(&f, v1).await;
        let before = support::all_rows(&f.store).await;
        for key in ["original", "new-intent"] {
            assert!(matches!(
                f.admit(key).await,
                Err(SandboxControlServiceError::Store(
                    SandboxControlError::Conflict
                ))
            ));
            assert_eq!(support::all_rows(&f.store).await, before);
        }
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(
            reopened.apply_sandbox_control(&record, proof).await,
            Err(SandboxControlError::Conflict)
        );
        assert_eq!(support::all_rows(&f.store).await, before);
        assert_eq!(
            reopened
                .reject_sandbox_control(&record, SandboxControlFailure::QueueExpired)
                .await,
            Err(SandboxControlError::Conflict)
        );
        assert_eq!(support::all_rows(&f.store).await, before);
        assert_eq!(
            reopened
                .sandbox_control(&record.envelope.command_id)
                .await
                .unwrap(),
            Some(record)
        );
        assert_eq!(support::all_rows(&f.store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and control hooks"]
async fn missing_hold_audits_refuse_receipt_only_rejection_without_writes() {
    for v1 in [false, true] {
        let (client, name) = database("maintenance_controls_audit").await;
        let f = retained_fixture(&client, &name).await;
        let record = f.admit("original").await.unwrap();
        hold(&f, v1).await;
        let audits = if v1 {
            crate::store::ledger_maintenance_transaction::audit_collection(&f.store)
        } else {
            crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&f.store)
        };
        audits.delete_many(doc! {}).await.unwrap();
        let before = support::all_rows(&f.store).await;
        assert_eq!(
            f.store
                .reject_sandbox_control(&record, SandboxControlFailure::AuthorityRevoked)
                .await,
            Err(SandboxControlError::Corrupt)
        );
        assert_eq!(support::all_rows(&f.store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and control hooks"]
async fn failed_control_receipt_rolls_back_parent_barrier_lifecycle_and_audit() {
    let (client, name) = database("maintenance_controls_rollback").await;
    let f = retained_fixture(&client, &name).await;
    let record = f.admit("original").await.unwrap();
    let proof = f.proof(&record, MAX_MEMBERSHIP_AGE).await;
    client
        .database(&name)
        .run_command(doc! {
            "collMod":f.store.sandbox_controls.name(),
            "validator":{"reject_fixture":{"$exists":true}},
            "validationLevel":"strict", "validationAction":"error",
        })
        .await
        .unwrap();
    let before = support::all_rows(&f.store).await;
    assert_eq!(
        f.store.apply_sandbox_control(&record, proof.clone()).await,
        Err(SandboxControlError::Unavailable)
    );
    assert_eq!(support::all_rows(&f.store).await, before);
    client
        .database(&name)
        .run_command(doc! {
            "collMod":f.store.sandbox_controls.name(), "validator":{},
        })
        .await
        .unwrap();
    let result = f.store.apply_sandbox_control(&record, proof).await.unwrap();
    assert!(matches!(
        result.status,
        SandboxControlStatus::Applied { .. }
    ));
    assert_ne!(support::all_rows(&f.store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and control hooks"]
async fn queued_control_rejection_cannot_write_an_orphan_or_recreated_parent() {
    for missing in [false, true] {
        let (client, name) = database("maintenance_controls_custody").await;
        let f = retained_fixture(&client, &name).await;
        let record = f.admit("original").await.unwrap();
        if missing {
            f.store.sandboxes.delete_many(doc! {}).await.unwrap();
        } else {
            let mut replacement = f.parent.clone();
            replacement.created_at = "2026-09-01T00:00:00Z".into();
            f.store
                .sandboxes
                .replace_one(
                    doc! {"_id":replacement.id.as_str()},
                    crate::store::sandboxes::sandbox_document(&replacement, 0).unwrap(),
                )
                .await
                .unwrap();
        }
        let before = support::all_rows(&f.store).await;
        let outcome = f
            .store
            .reject_sandbox_control(&record, SandboxControlFailure::TargetChanged)
            .await;
        if missing {
            assert_eq!(outcome, Err(SandboxControlError::NotFound));
        } else {
            assert!(matches!(
                outcome,
                Err(SandboxControlError::Corrupt | SandboxControlError::Conflict)
            ));
        }
        assert_eq!(support::all_rows(&f.store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and control hooks"]
async fn committed_receipt_only_rejection_invalidates_an_older_hold_transaction() {
    use crate::store::ledger_maintenance::{ApplyError, Change};
    use crate::store::sandbox_mongo::{is_transient, start_transaction};
    let (client, name) = database("maintenance_controls_race").await;
    let f = retained_fixture(&client, &name).await;
    let record = f.admit("original").await.unwrap();
    let input = v1_request(&f).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    f.store
        .sandboxes
        .find_one(doc! {"_id":f.parent.id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let result = other
        .reject_sandbox_control(&record, SandboxControlFailure::AuthorityRevoked)
        .await
        .unwrap();
    assert!(matches!(
        result.status,
        SandboxControlStatus::Rejected { .. }
    ));
    let before = support::all_rows(&f.store).await;
    let change = Change::Admit(LedgerMaintenanceAdmissionPolicy { enabled: true }, input);
    let outcome =
        crate::store::ledger_maintenance_transaction::apply(&f.store, &change, &mut session).await;
    assert!(matches!(outcome, Err(ApplyError::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    assert_eq!(support::all_rows(&f.store).await, before);
    client.database(&name).drop().await.unwrap();
}
