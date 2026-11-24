use super::lifecycle_test_support::NOW;
use super::runtime_update_test_support::native;
use super::store::{
    validate_lifecycle_mutation, SandboxLifecycleAgentScope, SandboxLifecycleMutation,
    SandboxMutationGuard,
};
use super::*;
const HOLDER: &str = "runtime-update-test";
const END: &str = "2026-09-05T01:00:00Z";

pub(super) fn step(
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
                    runtime_version: "image-v2".into(),
                }
            });
        }
    }
    let mutation = SandboxLifecycleMutation {
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
    };
    validate_lifecycle_mutation(&previous, &[], &mutation).unwrap();
    let bytes = serde_json::to_vec(value).unwrap();
    *value = serde_json::from_slice(&bytes).unwrap();
}
pub(super) fn pause(value: &mut Sandbox) {
    value
        .begin_pause(NOW.into(), Some("Paused by you.".into()))
        .unwrap();
    pause_to_completion(value);
}
fn pause_to_completion(value: &mut Sandbox) {
    step(value, |v, f| {
        v.complete_lifecycle_persistent_pause(HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.complete_lifecycle_agent_pause(HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.complete_lifecycle_runtime_suspension(HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.publish_lifecycle_paused(HOLDER, f, NOW.into())
    });
}
pub(super) fn admit(value: &mut Sandbox) {
    value
        .request_runtime_update("image-v2".into(), false, NOW.into())
        .unwrap();
    let identity = SandboxRotationIdentity::capture(value, &[]).unwrap();
    value
        .begin_image_rotation("image-v2".into(), identity, NOW.into())
        .unwrap();
}
pub(super) fn provision(value: &mut Sandbox) {
    let old = value.lifecycle.suspended_runtime.clone().unwrap();
    let home = value.lifecycle.home.clone();
    let source = value.retained_home_recovery().unwrap().clone();
    step(value, |v, f| {
        v.complete_lifecycle_home_verification(&source, HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.reserve_lifecycle_runtime_generation(2, HOLDER, f, NOW.into())
            .map(|_| ())
    });
    let binding = RuntimeBinding {
        runtime_id: old.runtime_id.clone(),
        runtime_version: "image-v2".into(),
        boot_id: Some("c".repeat(32)),
        provision_generation: Some(value.lifecycle.runtime_generation_high_water),
        bound_at: NOW.into(),
    };
    let mut wrong = binding.clone();
    wrong.runtime_id = "foreign".into();
    assert!(value.validate_resumed_binding(&wrong).is_err());
    wrong = binding.clone();
    wrong.runtime_version = "image-v3".into();
    assert!(value.validate_resumed_binding(&wrong).is_err());
    step(value, |v, f| {
        v.bind_lifecycle_runtime(binding, HOLDER, f, NOW.into())
    });
    assert_eq!(value.lifecycle.home, home);
    assert!(value.lifecycle.suspended_runtime.is_none());
}
fn finish(value: &mut Sandbox) {
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

#[test]
fn runtime_update_preserves_home_crd_across_all_restart_boundaries() {
    let mut value = native();
    let before = value.clone();
    admit(&mut value);
    step(&mut value, |v, f| {
        v.complete_lifecycle_persistent_pause(HOLDER, f, NOW.into())
    });
    step(&mut value, |v, f| {
        v.complete_lifecycle_agent_pause(HOLDER, f, NOW.into())
    });
    step(&mut value, |v, f| {
        v.complete_lifecycle_runtime_suspension(HOLDER, f, NOW.into())
    });
    assert_eq!(
        value.lifecycle.suspended_runtime.as_deref(),
        before.runtime.as_ref()
    );
    provision(&mut value);
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Active);
    assert_eq!(value.lifecycle.home, before.lifecycle.home);
    assert_eq!(
        value.runtime.as_ref().unwrap().runtime_id,
        before.runtime.unwrap().runtime_id
    );
    assert_eq!(
        value
            .runtime_update_request()
            .unwrap()
            .completed_at
            .as_deref(),
        Some(NOW)
    );
    assert_eq!(value.profile, before.profile);
}

#[test]
fn runtime_update_returns_originally_paused_sandbox_to_paused_on_same_home() {
    let mut value = native();
    pause(&mut value);
    let before = value.clone();
    admit(&mut value);
    assert!(value
        .retained_home_recovery()
        .unwrap()
        .resume_pause
        .is_some());
    provision(&mut value);
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Pausing);
    pause_to_completion(&mut value);
    assert_eq!(value.status, SandboxStatus::Paused);
    assert_eq!(value.lifecycle.home, before.lifecycle.home);
    assert_eq!(
        value.retained_runtime_binding().unwrap().runtime_version,
        "image-v2"
    );
}

#[test]
fn runtime_update_new_owner_work_supersedes_return_to_paused_intent() {
    let mut value = native();
    pause(&mut value);
    admit(&mut value);
    assert!(value.cancel_recovery_pause(NOW).unwrap());
    provision(&mut value);
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Active);
}

#[test]
fn runtime_update_failure_retries_same_exact_phase_and_old_pinned_target() {
    let mut value = native();
    admit(&mut value);
    let operation = value.lifecycle.operation.clone();
    let fence = value.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    let before = value.clone();
    let failure = TypedFailure {
        class: "runtime_update_failed".into(),
        detail: "Fixture failure".into(),
    };
    value
        .park_failed_runtime_update(failure.clone(), HOLDER, fence, NOW.into())
        .unwrap();
    let mut mutation = SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(&before),
        sandbox: value.clone(),
        agents: vec![],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: NOW.into(),
        events: vec![SandboxEventKind::WorkStepFailed {
            step: SandboxWorkStep::RunLifecycleOperation,
            failure: failure.clone(),
        }],
    };
    validate_lifecycle_mutation(&before, &[], &mutation).unwrap();
    assert_eq!(value.status, SandboxStatus::Rotating);
    mutation.events = vec![SandboxEventKind::Failed { failure }];
    assert!(validate_lifecycle_mutation(&before, &[], &mutation).is_err());
    assert_eq!(value.work.step, SandboxWorkStep::None);
    assert_eq!(value.lifecycle.operation, operation);
    assert!(value
        .request_runtime_update("image-v2".into(), false, NOW.into())
        .unwrap());
    assert_eq!(value.lifecycle.operation, operation);
    assert_eq!(value.work.step, SandboxWorkStep::RunLifecycleOperation);
    assert!(value.runtime_update_request().unwrap().failure.is_none());
}

#[test]
fn runtime_update_process_loss_retains_same_home_and_explicit_target() {
    let mut value = native();
    admit(&mut value);
    let home = value.lifecycle.home.clone();
    let binding = value.runtime.clone();
    value.recover_runtime_loss(&[], true, NOW.into()).unwrap();
    assert_eq!(value.lifecycle.home, home);
    assert_eq!(
        value.lifecycle.suspended_runtime.as_deref(),
        binding.as_ref()
    );
    assert!(
        matches!(value.lifecycle.operation, Some(SandboxLifecycleOperation::ImageRotation { phase: SandboxImageRotationPhase::CheckpointVerification, ref target_runtime_version, .. }) if target_runtime_version == "image-v2")
    );
    assert!(value
        .runtime_update_request()
        .unwrap()
        .completed_at
        .is_none());
}

#[test]
fn parked_runtime_update_can_retire_without_retrying_or_releasing_home() {
    let mut value = native();
    admit(&mut value);
    let fence = value.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    value
        .park_failed_runtime_update(
            TypedFailure {
                class: "runtime_update_failed".into(),
                detail: "Fixture failure".into(),
            },
            HOLDER,
            fence,
            NOW.into(),
        )
        .unwrap();
    assert_eq!(value.work.step, SandboxWorkStep::None);
    assert!(value.lifecycle.operation.is_some());
    let parked = value.clone();
    value.begin_retirement(NOW.into()).unwrap();
    value.validate().unwrap();
    assert_eq!(value.status, SandboxStatus::Retiring);
    assert_eq!(value.work.step, SandboxWorkStep::StopAgents);
    assert!(value.work.lease.is_none());
    assert!(value.lifecycle.operation.is_none());
    assert_eq!(value.lifecycle.home, parked.lifecycle.home);
    assert_eq!(value.runtime, parked.runtime);
    assert_eq!(
        value.lifecycle.suspended_runtime,
        parked.lifecycle.suspended_runtime
    );
    assert_eq!(
        value.runtime_update_request(),
        parked.runtime_update_request()
    );
    assert_eq!(value.revision, parked.revision + 1);
}

#[test]
fn runtime_update_on_resume_binds_latest_and_completes_the_original_home() {
    let mut value = native();
    pause(&mut value);
    let original = value.clone();
    value
        .begin_revive_with_managed_runtime(
            SandboxReviveCause::ExplicitRequest,
            Some("image-v2"),
            NOW.into(),
        )
        .unwrap();
    assert_eq!(value.admitted_runtime_update_target(), Some("image-v2"));
    provision(&mut value);
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Active);
    assert_eq!(value.lifecycle.home, original.lifecycle.home);
    assert_eq!(
        value.runtime.as_ref().unwrap().runtime_id,
        original.retained_runtime_binding().unwrap().runtime_id
    );
    assert!(value
        .runtime_update_request()
        .unwrap()
        .completed_at
        .is_some());
}

#[test]
fn runtime_update_on_resume_failure_retries_exact_phase_after_deploy_changes() {
    let mut value = native();
    pause(&mut value);
    value
        .begin_revive_with_managed_runtime(
            SandboxReviveCause::ExplicitRequest,
            Some("image-v2"),
            NOW.into(),
        )
        .unwrap();
    provision(&mut value);
    let operation = value.lifecycle.operation.clone();
    let fence = value.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    let before = value.clone();
    let failure = TypedFailure {
        class: "runtime_update_failed".into(),
        detail: "Fixture failure".into(),
    };
    value
        .park_failed_runtime_update(failure.clone(), HOLDER, fence, NOW.into())
        .unwrap();
    validate_lifecycle_mutation(
        &before,
        &[],
        &SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&before),
            sandbox: value.clone(),
            agents: vec![],
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: NOW.into(),
            events: vec![SandboxEventKind::WorkStepFailed {
                step: SandboxWorkStep::RunLifecycleOperation,
                failure,
            }],
        },
    )
    .unwrap();
    assert!(value
        .request_runtime_update("image-v3".into(), false, NOW.into())
        .is_err());
    assert!(value
        .request_runtime_update("image-v2".into(), false, NOW.into())
        .unwrap());
    assert_eq!(value.lifecycle.operation, operation);
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Active);
    assert_eq!(value.runtime.unwrap().runtime_version, "image-v2");
}
