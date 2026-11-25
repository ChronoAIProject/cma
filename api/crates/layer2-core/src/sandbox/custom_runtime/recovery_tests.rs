use super::test_support::*;
use crate::sandbox::lifecycle_test_support::NOW;
use crate::sandbox::*;
use std::time::{Duration, UNIX_EPOCH};

const RECOVERY_AT: &str = "2026-09-05T00:32:01Z";

#[test]
fn custom_runtime_unpublished_recovery_preserves_admitted_pin_and_explicit_request() {
    let mut value = custom();
    queue(&mut value, 'b', "migration-1");
    admit(&mut value, 'b');
    suspend(&mut value);
    provision(&mut value, 'b');
    step(&mut value, |v, f| {
        v.complete_lifecycle_credential_delivery(HOLDER, f, NOW.into())
    });
    step(&mut value, |v, f| {
        v.complete_lifecycle_home_restore(HOLDER, f, NOW.into())
    });
    let fence = value.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    value
        .park_failed_runtime_update(
            TypedFailure {
                class: "runtime_update_failed".into(),
                detail: "reopen failed".into(),
            },
            HOLDER,
            fence,
            NOW.into(),
        )
        .unwrap();
    assert!(value.unpublished_recovery_candidate());
    let original = value.clone();
    let snapshot = SandboxUnpublishedRecoverySnapshot {
        sandbox: value,
        agents: vec![],
        workspaces: vec![],
    };
    snapshot.validate().unwrap();
    let binding = snapshot.sandbox.retained_runtime_binding().unwrap();
    let plan = serde_json::json!({"inspection":"fixture"});
    let request = SandboxUnpublishedRecoveryRequest {
        proof: SandboxUnpublishedRecoveryProof {
            sandbox_id: snapshot.sandbox.id.clone(),
            runtime_id: binding.runtime_id.clone(),
            runtime_generation: binding.provision_generation.unwrap(),
            reserved_provision: None,
            observed_provision_generation: None,
            home_identity: snapshot.home_identity().unwrap(),
            boot_id: binding.boot_id.clone(),
            runtime_process_absent: true,
            requires_native_quiescence: true,
            provider_plan_sha256: SandboxUnpublishedRecoveryProof::provider_plan_digest(&plan)
                .unwrap(),
            provider_plan: plan,
            conversations: vec![],
        },
        snapshot: snapshot.clone(),
        target_runtime_version: image('b'),
        released_runtime: None,
        automatic: true,
        requested_at: RECOVERY_AT.into(),
    };
    let mut recovered = apply_unpublished_runtime_recovery(&snapshot, &request)
        .unwrap()
        .sandbox;
    assert_eq!(
        recovered.effective_custom_runtime_image(),
        Some(image('a').as_str())
    );
    assert_eq!(
        recovered.custom_runtime_image_for_binding().unwrap(),
        Some(image('b').as_str())
    );
    assert_eq!(
        recovered.lifecycle.custom_runtime,
        original.lifecycle.custom_runtime
    );
    assert_eq!(
        recovered.runtime_update_request().unwrap().requested_at,
        original.runtime_update_request().unwrap().requested_at
    );
    assert!(!recovered.runtime_update_request().unwrap().automatic);
    assert_eq!(
        recovered.runtime_update_request().unwrap().started_at,
        original.runtime_update_request().unwrap().started_at
    );
    let fence = recovered
        .work
        .claim(HOLDER.into(), RECOVERY_AT, END.into())
        .unwrap();
    recovered
        .park_failed_runtime_update(
            TypedFailure {
                class: "runtime_update_failed".into(),
                detail: "fixture".into(),
            },
            HOLDER,
            fence,
            RECOVERY_AT.into(),
        )
        .unwrap();
    let started = crate::seconds_from_rfc3339(RECOVERY_AT).unwrap();
    for elapsed in [0, 89, 90, 100] {
        let observed = SandboxLifecycleResult::observe(
            &recovered,
            None,
            UNIX_EPOCH + Duration::from_secs(started + elapsed),
            false,
        )
        .unwrap();
        assert_eq!(observed.state, SandboxLifecycleResultState::Failed);
        assert_eq!(observed.code, Some("runtime_update_parked"));
        assert_eq!(observed.started_at.as_deref(), Some(RECOVERY_AT));
        assert!(!observed.continuing);
        assert_eq!(observed.retry_after_seconds, None);
    }
    let mut mismatched = recovered.clone();
    mismatched
        .lifecycle
        .unpublished_recovery
        .as_mut()
        .unwrap()
        .operation_id = "different-operation".into();
    let overdue = UNIX_EPOCH + Duration::from_secs(started + 100);
    let observed = SandboxLifecycleResult::observe(&mismatched, None, overdue, false).unwrap();
    assert_eq!(
        observed.state,
        SandboxLifecycleResultState::RecoverablePending
    );
    assert_eq!(observed.retry_after_seconds, Some(5));
    let mut wrong = request;
    wrong.automatic = false;
    wrong.target_runtime_version = image('c');
    assert!(apply_unpublished_runtime_recovery(&snapshot, &wrong).is_err());
}

#[test]
fn custom_runtime_unpublished_recovery_does_not_authorize_first_custom_migration() {
    let mut value = custom();
    value.status = SandboxStatus::Failed;
    value.failure = Some(TypedFailure {
        class: "restore_failed".into(),
        detail: "fixture".into(),
    });
    assert!(!value.supports_retained_custom_runtime_recovery());
    assert!(!value.unpublished_recovery_candidate());
}
