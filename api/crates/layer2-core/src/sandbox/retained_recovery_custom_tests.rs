use super::*;
use crate::sandbox::{
    SandboxCustomRuntimeMigration, SandboxCustomRuntimeOrigin, SandboxRotationIdentity,
};

const HOLDER: &str = "custom-recovery-test";

fn step(sandbox: &mut Sandbox, action: impl FnOnce(&mut Sandbox, u64)) {
    let fence = sandbox
        .work
        .claim(HOLDER.into(), NOW, "2026-09-05T01:00:00Z".into())
        .unwrap();
    action(sandbox, fence);
    sandbox.validate().unwrap();
    *sandbox = serde_json::from_slice(&serde_json::to_vec(sandbox).unwrap()).unwrap();
}

fn migrated() -> RetainedRecoverySnapshot {
    let mut snapshot = snapshot();
    let sandbox = &mut snapshot.sandbox;
    sandbox.status = SandboxStatus::Active;
    sandbox.failure = None;
    let original = sandbox.runtime.as_ref().unwrap().runtime_version.clone();
    sandbox.profile.content.runtime_image = Some(original.clone());
    sandbox.profile.digest = crate::profiles::content_digest(&sandbox.profile.content).unwrap();
    let target = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let request = SandboxCustomRuntimeMigration {
        request_key: "custom-recovery".into(),
        expected_revision: sandbox.revision,
        expected_image: original,
        target_image: target.clone(),
        origin: SandboxCustomRuntimeOrigin::capture(sandbox).unwrap(),
    };
    sandbox
        .request_custom_runtime_migration(request, &target, NOW.into())
        .unwrap();
    let identity = SandboxRotationIdentity::capture(sandbox, &[]).unwrap();
    sandbox
        .begin_image_rotation(target.clone(), identity, NOW.into())
        .unwrap();
    step(sandbox, |s, f| {
        s.complete_lifecycle_persistent_pause(HOLDER, f, NOW.into())
            .unwrap();
    });
    step(sandbox, |s, f| {
        s.complete_lifecycle_agent_pause(HOLDER, f, NOW.into())
            .unwrap();
    });
    step(sandbox, |s, f| {
        s.complete_lifecycle_runtime_suspension(HOLDER, f, NOW.into())
            .unwrap();
    });
    let source = sandbox.retained_home_recovery().unwrap().clone();
    step(sandbox, |s, f| {
        s.complete_lifecycle_home_verification(&source, HOLDER, f, NOW.into())
            .unwrap();
    });
    step(sandbox, |s, f| {
        s.reserve_lifecycle_runtime_generation(2, HOLDER, f, NOW.into())
            .unwrap();
    });
    let mut binding = sandbox
        .lifecycle
        .suspended_runtime
        .as_deref()
        .unwrap()
        .clone();
    binding.runtime_version = target;
    binding.provision_generation = Some(2);
    binding.boot_id = Some("c".repeat(32));
    binding.bound_at = NOW.into();
    step(sandbox, |s, f| {
        s.bind_lifecycle_runtime(binding, HOLDER, f, NOW.into())
            .unwrap();
    });
    step(sandbox, |s, f| {
        s.complete_lifecycle_credential_delivery(HOLDER, f, NOW.into())
            .unwrap();
    });
    step(sandbox, |s, f| {
        s.complete_lifecycle_home_restore(HOLDER, f, NOW.into())
            .unwrap();
    });
    step(sandbox, |s, f| {
        s.complete_lifecycle_conversation_reopen(HOLDER, f, NOW.into())
            .unwrap();
    });
    step(sandbox, |s, f| {
        s.complete_lifecycle_queued_message_delivery(HOLDER, f, NOW.into())
            .unwrap();
    });
    step(sandbox, |s, f| {
        s.publish_lifecycle_active("2026-09-05T01:00:00Z".into(), HOLDER, f, NOW.into())
            .unwrap();
    });
    sandbox.status = SandboxStatus::Failed;
    sandbox.failure = Some(TypedFailure {
        class: "restore_failed".into(),
        detail: "runtime process stopped".into(),
    });
    sandbox.work.schedule(SandboxWorkStep::None, NOW.into());
    snapshot.agents[0].conversation.runtime_generation = Some(2);
    snapshot
}

#[test]
fn retained_recovery_uses_completed_custom_pin_without_rewriting_saved_profile() {
    let held = migrated();
    let before = held.clone();
    let source = held.source().unwrap();
    assert_eq!(
        Some(source.binding.runtime_version.as_str()),
        held.sandbox.effective_custom_runtime_image()
    );
    assert_ne!(
        held.sandbox.profile.content.runtime_image.as_deref(),
        held.sandbox.effective_custom_runtime_image()
    );
    assert!(!held.sandbox.supports_retained_runtime_update());
    assert_eq!(held, before);
}

#[test]
fn retained_recovery_refuses_changed_custom_pin_or_unapproved_binding() {
    let held = migrated();
    for change_pin in [false, true] {
        let mut changed = held.clone();
        if change_pin {
            changed
                .sandbox
                .lifecycle
                .custom_runtime
                .as_mut()
                .unwrap()
                .effective_image = changed
                .sandbox
                .profile
                .content
                .runtime_image
                .clone()
                .unwrap();
        } else {
            changed.sandbox.runtime.as_mut().unwrap().runtime_version =
                format!("registry.example/runtime@sha256:{}", "d".repeat(64));
        }
        let before = changed.clone();
        assert!(changed.source().is_err());
        assert_eq!(changed, before);
    }
}
