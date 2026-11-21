use crate::sandbox::lifecycle_test_support::{sandbox, NOW};
use crate::sandbox::store::{
    validate_lifecycle_mutation, SandboxLifecycleAgentScope, SandboxLifecycleMutation,
    SandboxMutationGuard,
};
use crate::sandbox::*;
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

const HOLDER: &str = "suspension-worker";
const END: &str = "2026-09-05T01:00:00Z";

fn claimed(value: &mut Sandbox) -> u64 {
    value.work.claim(HOLDER.into(), NOW, END.into()).unwrap()
}

fn ready_to_suspend() -> Sandbox {
    let mut value = sandbox();
    let mut home = SandboxHome::reserve(
        &value.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "storage".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "original-pvc".into(),
        storage_generation: "pvc-uid".into(),
    })
    .unwrap();
    value.lifecycle.home = Some(Box::new(home));
    value.lifecycle.runtime_generation_high_water = 1;
    let runtime = value.runtime.as_mut().unwrap();
    runtime.provision_generation = Some(1);
    runtime.boot_id = Some("1".repeat(32));
    value.begin_pause(NOW.into(), None).unwrap();
    let fence = claimed(&mut value);
    value
        .complete_lifecycle_persistent_pause(HOLDER, fence, NOW.into())
        .unwrap();
    let fence = claimed(&mut value);
    value
        .complete_lifecycle_agent_pause(HOLDER, fence, NOW.into())
        .unwrap();
    claimed(&mut value);
    value
}

fn suspended() -> Sandbox {
    let mut value = ready_to_suspend();
    let fence = value.work.lease.as_ref().unwrap().fence;
    value
        .complete_lifecycle_runtime_suspension(HOLDER, fence, NOW.into())
        .unwrap();
    let fence = claimed(&mut value);
    value
        .publish_lifecycle_paused(HOLDER, fence, NOW.into())
        .unwrap();
    value
}

#[test]
fn suspension_keeps_exact_runtime_and_home_authority_and_rejects_forged_publication() {
    let current = ready_to_suspend();
    let mut next = current.clone();
    let fence = current.work.lease.as_ref().unwrap().fence;
    assert!(next
        .complete_lifecycle_runtime_suspension(HOLDER, fence + 1, NOW.into())
        .is_err());
    assert_eq!(next, current);
    next.complete_lifecycle_runtime_suspension(HOLDER, fence, NOW.into())
        .unwrap();
    assert_eq!(
        next.lifecycle.suspended_runtime.as_deref(),
        current.runtime.as_ref()
    );
    assert_eq!(next.lifecycle.home, current.lifecycle.home);
    assert!(next.runtime.is_none());
    let mutation = SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(&current),
        sandbox: next.clone(),
        agents: vec![],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: NOW.into(),
        events: vec![SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }],
    };
    validate_lifecycle_mutation(&current, &[], &mutation).unwrap();
    let mut forged = mutation;
    forged
        .sandbox
        .lifecycle
        .suspended_runtime
        .as_mut()
        .unwrap()
        .runtime_id = "foreign".into();
    assert!(validate_lifecycle_mutation(&current, &[], &forged).is_err());
    assert_eq!(
        serde_json::from_slice::<Sandbox>(&serde_json::to_vec(&next).unwrap()).unwrap(),
        next
    );
}

#[test]
fn resumption_requires_same_crd_and_image_but_new_generation_and_process() {
    let mut value = suspended();
    let previous = value
        .lifecycle
        .suspended_runtime
        .as_deref()
        .unwrap()
        .clone();
    value
        .begin_revive(SandboxReviveCause::ExplicitRequest, NOW.into())
        .unwrap();
    let source = value.retained_home_recovery().unwrap().clone();
    let fence = claimed(&mut value);
    value
        .complete_lifecycle_home_verification(&source, HOLDER, fence, NOW.into())
        .unwrap();
    let fence = claimed(&mut value);
    let generation = value
        .reserve_lifecycle_runtime_generation(2, HOLDER, fence, NOW.into())
        .unwrap();
    let fence = claimed(&mut value);
    let resumed = RuntimeBinding {
        boot_id: Some("2".repeat(32)),
        provision_generation: Some(generation),
        bound_at: NOW.into(),
        ..previous.clone()
    };
    for field in 0..4 {
        let mut bad = resumed.clone();
        match field {
            0 => bad.runtime_id = "another-crd".into(),
            1 => bad.runtime_version = "another-image".into(),
            2 => bad.provision_generation = previous.provision_generation,
            _ => bad.boot_id = previous.boot_id.clone(),
        }
        let before = value.clone();
        assert!(value
            .bind_lifecycle_runtime(bad, HOLDER, fence, NOW.into())
            .is_err());
        assert_eq!(value, before);
    }
    value
        .bind_lifecycle_runtime(resumed.clone(), HOLDER, fence, NOW.into())
        .unwrap();
    assert_eq!(value.runtime.take(), Some(resumed.clone()));
    assert!(value.lifecycle.suspended_runtime.is_none());
}

#[test]
fn retiring_a_paused_sandbox_keeps_its_crd_until_delete_finality() {
    let mut value = suspended();
    let retained = value.lifecycle.suspended_runtime.clone();
    value.begin_retirement(NOW.into()).unwrap();
    value.validate().unwrap();
    assert_eq!(value.lifecycle.suspended_runtime, retained);
    value.status = SandboxStatus::Retired;
    assert!(value.validate().is_err());
}

#[test]
fn retirement_preempting_native_suspension_does_not_invent_archive_writer_debt() {
    let mut value = ready_to_suspend();
    let original = value.runtime.clone();
    value.begin_retirement(NOW.into()).unwrap();
    value.validate().unwrap();
    assert_eq!(value.runtime, original);
    assert!(value.lifecycle.checkpoint_capture.is_none());
    assert!(value.lifecycle.background_checkpoint.is_none());
    assert!(value.work.is_due(NOW));
}
