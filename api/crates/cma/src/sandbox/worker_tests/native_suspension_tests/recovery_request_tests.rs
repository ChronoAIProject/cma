use super::*;
use crate::sandbox::runtime_attachment::{
    recovery_runtime_request, reserved_recovery_runtime_request, retained_runtime_request,
};
use layer2_core::sandbox::{SandboxUnpublishedRecoveryVerifier, SandboxWorkStep, TypedFailure};

#[tokio::test]
async fn exact_reserved_request_retains_all_source_fields_and_original_generation() {
    let (_, snapshot, _, _) = nonexecuting_recovery_tests::stalled(Some(2)).await;
    let binding = snapshot.sandbox.unpublished_recovery_binding().unwrap();
    let original = retained_runtime_request(&snapshot.sandbox, binding, "test").unwrap();
    let reserved = reserved_recovery_runtime_request(&snapshot, "test")
        .unwrap()
        .unwrap();
    assert_eq!(original.generation, 1);
    assert_eq!(reserved.generation, 2);
    assert_eq!(original.handle, reserved.handle);
    assert_eq!(original.spec, reserved.spec);
    assert_eq!(original.expected_image, reserved.expected_image);
    assert_eq!(binding.provision_generation, Some(1));
    assert_eq!(
        snapshot
            .sandbox
            .retained_home_recovery()
            .unwrap()
            .source_generation,
        1
    );
}

#[tokio::test]
async fn recovery_request_reloads_reserved_evidence_without_relabeling_retained_source() {
    let (_, sandbox, _, _) = nonexecuting_recovery_tests::admitted_generation(Some(2)).await;
    let decoded: Sandbox = serde_json::from_slice(&serde_json::to_vec(&sandbox).unwrap()).unwrap();
    let binding = decoded.lifecycle.suspended_runtime.as_deref().unwrap();
    assert_eq!(
        retained_runtime_request(&decoded, binding, "test")
            .unwrap()
            .generation,
        1
    );
    assert_eq!(
        recovery_runtime_request(&decoded, binding, "test")
            .unwrap()
            .generation,
        2
    );
    assert_eq!(decoded, sandbox);
    for change in ["operation", "generation", "floor", "home"] {
        let mut changed = decoded.clone();
        match change {
            "operation" => {
                changed
                    .lifecycle
                    .unpublished_recovery
                    .as_mut()
                    .unwrap()
                    .proof
                    .reserved_provision
                    .as_mut()
                    .unwrap()
                    .operation_id = "foreign".into()
            }
            "generation" => {
                changed
                    .lifecycle
                    .unpublished_recovery
                    .as_mut()
                    .unwrap()
                    .proof
                    .reserved_provision
                    .as_mut()
                    .unwrap()
                    .generation = 3
            }
            "floor" => changed.lifecycle.runtime_generation_high_water = 1,
            "home" => {
                changed
                    .lifecycle
                    .unpublished_recovery
                    .as_mut()
                    .unwrap()
                    .proof
                    .home_identity = "changed".into()
            }
            _ => unreachable!(),
        }
        assert!(
            recovery_runtime_request(&changed, binding, "test").is_err(),
            "{change}"
        );
    }
}

#[tokio::test]
async fn observed_generation_without_a_reservation_never_grants_recovery_authority() {
    let (fixture, mut snapshot, _, _) = nonexecuting_recovery_tests::stalled(Some(2)).await;
    let sandbox = &mut snapshot.sandbox;
    sandbox.runtime = sandbox
        .lifecycle
        .suspended_runtime
        .take()
        .map(|binding| *binding);
    sandbox.lifecycle.runtime_provision = None;
    sandbox.lifecycle.home_recovery = None;
    sandbox.lifecycle.operation = None;
    sandbox.lifecycle.paused_notice = None;
    sandbox.lifecycle.runtime_generation_high_water = 1;
    sandbox.status = SandboxStatus::Failed;
    sandbox.failure = Some(TypedFailure {
        class: "provisioning_failed".into(),
        detail: "original startup failed".into(),
    });
    sandbox
        .work
        .schedule(SandboxWorkStep::None, sandbox.updated_at.clone());
    sandbox.validate().unwrap();
    snapshot.validate().unwrap();
    assert!(reserved_recovery_runtime_request(&snapshot, "test")
        .unwrap()
        .is_none());
    let verifier = crate::sandbox::unpublished_recovery::UnpublishedRuntimeRecovery::new(
        fixture.adapter(&runtime_image()),
        "test".into(),
    );
    assert!(verifier.verify(&snapshot).await.is_err());
    assert_eq!(
        *fixture.backend.inspected_generations.lock().unwrap(),
        vec![1]
    );
    assert!(!fixture.backend.calls.snapshot().contains(&"suspend".into()));
}

#[tokio::test]
async fn changed_reservation_operation_and_high_water_refuse_before_provider_reads() {
    let (fixture, snapshot, _, _) = nonexecuting_recovery_tests::stalled(Some(2)).await;
    let verifier = crate::sandbox::unpublished_recovery::UnpublishedRuntimeRecovery::new(
        fixture.adapter(&runtime_image()),
        "test".into(),
    );
    for change in ["operation", "high-water", "missing", "source"] {
        let mut changed = snapshot.clone();
        match change {
            "operation" => {
                changed
                    .sandbox
                    .lifecycle
                    .runtime_provision
                    .as_mut()
                    .unwrap()
                    .operation_id = "foreign".into()
            }
            "high-water" => changed.sandbox.lifecycle.runtime_generation_high_water = 3,
            "missing" => changed.sandbox.lifecycle.runtime_provision = None,
            "source" => {
                changed
                    .sandbox
                    .lifecycle
                    .suspended_runtime
                    .as_mut()
                    .unwrap()
                    .provision_generation = Some(2)
            }
            _ => unreachable!(),
        }
        assert!(verifier.verify(&changed).await.is_err(), "{change}");
    }
    assert!(fixture
        .backend
        .inspected_generations
        .lock()
        .unwrap()
        .is_empty());
}
