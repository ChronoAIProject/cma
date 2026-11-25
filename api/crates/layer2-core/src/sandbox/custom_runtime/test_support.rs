use crate::sandbox::lifecycle_test_support::NOW;
use crate::sandbox::store::{
    validate_lifecycle_mutation, SandboxLifecycleAgentScope, SandboxLifecycleMutation,
    SandboxMutationGuard,
};
use crate::sandbox::*;

pub(in crate::sandbox) const HOLDER: &str = "custom-runtime-test";
pub(in crate::sandbox) const END: &str = "2026-09-05T01:00:00Z";

pub(in crate::sandbox) fn image(digit: char) -> String {
    format!(
        "registry.example/runtime@sha256:{}",
        digit.to_string().repeat(64)
    )
}

pub(in crate::sandbox) fn custom() -> Sandbox {
    let mut sandbox = crate::sandbox::runtime_update_test_support::native();
    sandbox.profile.content.runtime_image = Some(image('a'));
    sandbox.profile.digest = crate::profiles::content_digest(&sandbox.profile.content).unwrap();
    sandbox.runtime.as_mut().unwrap().runtime_version = image('a');
    sandbox.validate().unwrap();
    sandbox
}

pub(in crate::sandbox) fn request(
    sandbox: &Sandbox,
    digit: char,
    key: &str,
) -> SandboxCustomRuntimeMigration {
    SandboxCustomRuntimeMigration {
        request_key: key.into(),
        expected_revision: sandbox.revision,
        expected_image: sandbox.effective_custom_runtime_image().unwrap().into(),
        target_image: image(digit),
        origin: SandboxCustomRuntimeOrigin::capture(sandbox).unwrap(),
    }
}

pub(in crate::sandbox) fn queue(
    sandbox: &mut Sandbox,
    digit: char,
    key: &str,
) -> SandboxCustomRuntimeMigration {
    let request = request(sandbox, digit, key);
    assert!(sandbox
        .request_custom_runtime_migration(request.clone(), &image(digit), NOW.into())
        .unwrap());
    request
}

pub(in crate::sandbox) fn admit(sandbox: &mut Sandbox, digit: char) {
    let identity = SandboxRotationIdentity::capture(sandbox, &[]).unwrap();
    sandbox
        .begin_image_rotation(image(digit), identity, NOW.into())
        .unwrap();
}

pub(in crate::sandbox) fn step(
    value: &mut Sandbox,
    f: impl FnOnce(&mut Sandbox, u64) -> Result<(), SandboxModelError>,
) {
    let fence = value.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    let previous = value.clone();
    f(value, fence).unwrap();
    let agent_phase = matches!(
        previous.lifecycle.operation,
        Some(
            SandboxLifecycleOperation::Pause {
                phase: SandboxPausePhase::PauseAgents,
                ..
            } | SandboxLifecycleOperation::ImageRotation {
                phase: SandboxImageRotationPhase::PauseAgents
                    | SandboxImageRotationPhase::ConversationReopen
                    | SandboxImageRotationPhase::QueuedMessageDelivery,
                ..
            } | SandboxLifecycleOperation::Revive {
                phase: SandboxRevivePhase::ConversationReopen
                    | SandboxRevivePhase::QueuedMessageDelivery,
                ..
            }
        )
    );
    let mut events = vec![SandboxEventKind::WorkStepCompleted {
        step: SandboxWorkStep::RunLifecycleOperation,
    }];
    if previous.status != value.status {
        events = vec![SandboxEventKind::StatusChanged {
            from: previous.status,
            to: value.status,
        }];
        if value.status == SandboxStatus::Active {
            events.push(if previous.status == SandboxStatus::Reviving {
                SandboxEventKind::Revived
            } else {
                SandboxEventKind::Rotated {
                    runtime_version: value.runtime.as_ref().unwrap().runtime_version.clone(),
                }
            });
        }
    }
    validate_lifecycle_mutation(
        &previous,
        &[],
        &SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&previous),
            sandbox: value.clone(),
            agents: vec![],
            agent_scope: if agent_phase {
                SandboxLifecycleAgentScope::AllLive
            } else {
                SandboxLifecycleAgentScope::Touched
            },
            appended_at: NOW.into(),
            events,
        },
    )
    .unwrap();
    *value = serde_json::from_slice(&serde_json::to_vec(value).unwrap()).unwrap();
    value.validate().unwrap();
}

pub(in crate::sandbox) fn suspend(value: &mut Sandbox) {
    step(value, |v, f| {
        v.complete_lifecycle_persistent_pause(HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.complete_lifecycle_agent_pause(HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.complete_lifecycle_runtime_suspension(HOLDER, f, NOW.into())
    });
}

pub(in crate::sandbox) fn pause(value: &mut Sandbox) {
    value.begin_pause(NOW.into(), None).unwrap();
    suspend(value);
    step(value, |v, f| {
        v.publish_lifecycle_paused(HOLDER, f, NOW.into())
    });
}

pub(in crate::sandbox) fn provision(value: &mut Sandbox, digit: char) {
    let old = value.lifecycle.suspended_runtime.clone().unwrap();
    let source = value.retained_home_recovery().unwrap().clone();
    step(value, |v, f| {
        v.complete_lifecycle_home_verification(&source, HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.reserve_lifecycle_runtime_generation(
            v.lifecycle.runtime_generation_high_water + 1,
            HOLDER,
            f,
            NOW.into(),
        )
        .map(|_| ())
    });
    let generation = value.lifecycle.runtime_generation_high_water;
    let binding = RuntimeBinding {
        runtime_id: old.runtime_id.clone(),
        runtime_version: image(digit),
        boot_id: Some(format!("{generation:032x}")),
        provision_generation: Some(generation),
        bound_at: NOW.into(),
    };
    step(value, |v, f| {
        v.bind_lifecycle_runtime(binding, HOLDER, f, NOW.into())
    });
}

pub(in crate::sandbox) fn finish(value: &mut Sandbox) {
    step(value, |v, f| {
        v.complete_lifecycle_credential_delivery(HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.complete_lifecycle_home_restore(HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.complete_lifecycle_conversation_reopen(HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.complete_lifecycle_queued_message_delivery(HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.publish_lifecycle_active(END.into(), HOLDER, f, NOW.into())
    });
}
