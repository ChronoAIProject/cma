use super::test_support::*;
use crate::sandbox::lifecycle_test_support::NOW;
use crate::sandbox::*;

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
        automatic: true,
        requested_at: NOW.into(),
    };
    let recovered = apply_unpublished_runtime_recovery(&snapshot, &request)
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
