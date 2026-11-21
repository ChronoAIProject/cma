use super::*;
use crate::sandbox::{RuntimeBinding, SandboxPausePhase, SandboxRecoveryPause, SandboxStatus};

fn step(
    value: &mut Sandbox,
    f: impl FnOnce(&mut Sandbox, u64) -> Result<(), crate::sandbox::SandboxModelError>,
) {
    let fence = value.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    f(value, fence).unwrap();
    value.validate().unwrap();
    let bytes = serde_json::to_vec(value).unwrap();
    *value = serde_json::from_slice(&bytes).unwrap();
}

fn reach_reopen(value: &mut Sandbox) {
    if value.lifecycle.unpublished_recovery.is_some() {
        let receipt = quiescence(value);
        step(value, |v, f| {
            v.complete_unpublished_runtime_quiescence(receipt, HOLDER, f, NOW.into())
        });
    }
    let source = value.retained_home_recovery().unwrap().clone();
    step(value, |v, f| {
        v.complete_lifecycle_home_verification(&source, HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.reserve_lifecycle_runtime_generation(3, HOLDER, f, NOW.into())
            .map(|_| ())
    });
    let original = value.lifecycle.suspended_runtime.as_ref().unwrap();
    let binding = RuntimeBinding {
        runtime_id: original.runtime_id.clone(),
        runtime_version: "image-v3".into(),
        provision_generation: Some(value.lifecycle.runtime_generation_high_water),
        boot_id: Some(format!(
            "{:032x}",
            value.lifecycle.runtime_generation_high_water
        )),
        bound_at: NOW.into(),
    };
    step(value, |v, f| {
        v.bind_lifecycle_runtime(binding, HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.complete_lifecycle_credential_delivery(HOLDER, f, NOW.into())
    });
    step(value, |v, f| {
        v.complete_lifecycle_home_restore(HOLDER, f, NOW.into())
    });
}

fn finish(value: &mut Sandbox) {
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
fn terminal_only_recovery_finishes_without_reopening_deleted_agents() {
    let held = history::terminal_snapshot();
    let requested = request(held.clone());
    let original_agents = held.agents.clone();
    let mut value = apply_unpublished_runtime_recovery(&held, &requested)
        .unwrap()
        .sandbox;
    reach_reopen(&mut value);
    let Some(SandboxLifecycleOperation::ImageRotation { identity, .. }) =
        &value.lifecycle.operation
    else {
        panic!();
    };
    assert!(identity.agents.is_empty());
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Active);
    assert_eq!(
        value.runtime.as_ref().unwrap().provision_generation,
        Some(4)
    );
    assert_eq!(value.lifecycle.home, held.sandbox.lifecycle.home);
    assert_eq!(requested.snapshot.agents, original_agents);
    assert!(requested
        .snapshot
        .agents
        .iter()
        .all(|agent| agent.status == AgentStatus::Deleted));
}

#[test]
fn recovery_completes_all_phases_and_allows_future_pause_update_and_retirement() {
    let mut value = admitted();
    let home = value.lifecycle.home.clone();
    let profile = value.profile.clone();
    let original_conversations = value
        .lifecycle
        .unpublished_recovery
        .as_ref()
        .unwrap()
        .proof
        .conversations
        .clone();
    reach_reopen(&mut value);
    let Some(SandboxLifecycleOperation::ImageRotation { identity, .. }) =
        &value.lifecycle.operation
    else {
        panic!()
    };
    assert_eq!(
        identity.agents[0].conversation_id,
        original_conversations[0].conversation_id
    );
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Active);
    assert_eq!(
        value.runtime.as_ref().unwrap().provision_generation,
        Some(3)
    );
    assert_eq!(value.runtime.as_ref().unwrap().runtime_version, "image-v3");
    assert_eq!(value.lifecycle.home, home);
    assert_eq!(value.profile, profile);
    assert!(value.lifecycle.unpublished_recovery.is_none());
    assert!(!value.lifecycle.unpublished_recovery_attempted);
    assert!(value.lifecycle.operation.is_none());
    assert!(value
        .runtime_update_request()
        .unwrap()
        .completed_at
        .is_some());
    let mut paused = value.clone();
    paused.begin_pause(NOW.into(), None).unwrap();
    step(&mut paused, |v, f| {
        v.complete_lifecycle_persistent_pause(HOLDER, f, NOW.into())
    });
    step(&mut paused, |v, f| {
        v.complete_lifecycle_agent_pause(HOLDER, f, NOW.into())
    });
    step(&mut paused, |v, f| {
        v.complete_lifecycle_runtime_suspension(HOLDER, f, NOW.into())
    });
    step(&mut paused, |v, f| {
        v.publish_lifecycle_paused(HOLDER, f, NOW.into())
    });
    assert_eq!(paused.status, SandboxStatus::Paused);
    let mut update = value.clone();
    update
        .request_runtime_update("image-v4".into(), false, NOW.into())
        .unwrap();
    let identity = SandboxRotationIdentity::capture(&update, &[]).unwrap();
    update
        .begin_image_rotation("image-v4".into(), identity, NOW.into())
        .unwrap();
    update.validate().unwrap();
    value.begin_retirement(NOW.into()).unwrap();
    assert_eq!(value.status, SandboxStatus::Retiring);
    assert!(value.lifecycle.unpublished_recovery.is_none());
}

#[test]
fn lost_replacement_preserves_incident_bound_and_recovers_without_reusing_old_quiescence() {
    let mut value = admitted();
    reach_reopen(&mut value);
    assert!(value.lifecycle.unpublished_recovery.is_none());
    assert!(value.lifecycle.unpublished_recovery_attempted);
    let original_agents = snapshot(true).agents;
    value
        .recover_runtime_loss(&original_agents, true, NOW.into())
        .unwrap();
    assert!(value.lifecycle.unpublished_recovery.is_none());
    assert!(value.lifecycle.unpublished_recovery_attempted);
    assert_eq!(value.retained_home_recovery().unwrap().source_generation, 3);
    reach_reopen(&mut value);
    assert_eq!(
        value.runtime.as_ref().unwrap().provision_generation,
        Some(4)
    );
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Active);
    assert!(!value.lifecycle.unpublished_recovery_attempted);
}

#[test]
fn unresolved_recovery_allows_explicit_retry_but_cannot_admit_another_automatic_generation() {
    let mut value = admitted();
    reach_reopen(&mut value);
    let fence = value.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    assert!(value
        .fail_lifecycle("must retain incident".into(), HOLDER, fence, NOW.into())
        .is_err());
    value
        .park_failed_runtime_update(failure("restore_failed"), HOLDER, fence, NOW.into())
        .unwrap();
    let mut held = snapshot(true);
    held.sandbox = value;
    held.validate().unwrap();
    let mut requested = request(held.clone());
    requested.automatic = true;
    assert!(apply_unpublished_runtime_recovery(&held, &requested).is_err());
    requested.automatic = false;
    let retried = apply_unpublished_runtime_recovery(&held, &requested).unwrap();
    assert!(retried.sandbox.lifecycle.unpublished_recovery_attempted);
    let mut renewed = retried.sandbox;
    reach_reopen(&mut renewed);
    let fence = renewed.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    renewed
        .park_failed_runtime_update(failure("terminal_validation"), HOLDER, fence, NOW.into())
        .unwrap();
    assert_eq!(renewed.work.step, SandboxWorkStep::None);
    assert!(renewed.work.lease.is_none());
    assert!(renewed.lifecycle.unpublished_recovery_attempted);
    assert_eq!(
        renewed
            .runtime_update_request()
            .unwrap()
            .failure
            .as_ref()
            .unwrap()
            .class,
        "terminal_validation"
    );
    let mut parked_again = snapshot(true);
    parked_again.sandbox = renewed;
    parked_again.validate().unwrap();
    let mut automatic = request(parked_again.clone());
    automatic.automatic = true;
    assert!(apply_unpublished_runtime_recovery(&parked_again, &automatic).is_err());
    let mut bypass = held.sandbox.clone();
    bypass.lifecycle.unpublished_recovery_attempted = false;
    assert!(held
        .sandbox
        .validate_runtime_generation_successor(&bypass)
        .is_err());
    let mut first = request(snapshot(true));
    first.automatic = true;
    assert!(
        apply_unpublished_runtime_recovery(&first.snapshot, &first).is_err(),
        "automatic target cannot change"
    );
}

#[test]
fn recovered_rotation_can_return_to_original_pause_and_retire_before_quiescence() {
    let mut value = admitted();
    value.lifecycle.home_recovery.as_mut().unwrap().resume_pause = Some(SandboxRecoveryPause {
        notice: Some("Paused by you.".into()),
    });
    reach_reopen(&mut value);
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Pausing);
    assert!(value.lifecycle.unpublished_recovery.is_none());
    assert!(matches!(
        value.lifecycle.operation,
        Some(SandboxLifecycleOperation::Pause {
            phase: SandboxPausePhase::CaptureAndPublishCheckpoint,
            ..
        })
    ));
    step(&mut value, |v, f| {
        v.complete_lifecycle_persistent_pause(HOLDER, f, NOW.into())
    });
    step(&mut value, |v, f| {
        v.complete_lifecycle_agent_pause(HOLDER, f, NOW.into())
    });
    step(&mut value, |v, f| {
        v.complete_lifecycle_runtime_suspension(HOLDER, f, NOW.into())
    });
    step(&mut value, |v, f| {
        v.publish_lifecycle_paused(HOLDER, f, NOW.into())
    });
    assert_eq!(value.status, SandboxStatus::Paused);
    let mut retiring = admitted();
    retiring.begin_retirement(NOW.into()).unwrap();
    assert!(retiring.lifecycle.unpublished_recovery.is_none());
    retiring.validate().unwrap();
}

#[test]
fn receipt_only_transition_requires_exact_durable_audit_event() {
    use crate::sandbox::store::{
        validate_lifecycle_mutation, SandboxLifecycleAgentScope, SandboxLifecycleMutation,
        SandboxMutationGuard,
    };
    let mut value = admitted();
    let fence = value.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    let before = value.clone();
    let receipt = quiescence(&value);
    value
        .complete_unpublished_runtime_quiescence(receipt.clone(), HOLDER, fence, NOW.into())
        .unwrap();
    let mut mutation = SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(&before),
        sandbox: value,
        agents: vec![],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: NOW.into(),
        events: vec![SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }],
    };
    assert!(validate_lifecycle_mutation(&before, &[], &mutation).is_err());
    mutation.events.insert(
        0,
        SandboxEventKind::RuntimeRecoveryQuiesced {
            receipt: Box::new(receipt.clone()),
        },
    );
    validate_lifecycle_mutation(&before, &[], &mutation).unwrap();
    let SandboxEventKind::RuntimeRecoveryQuiesced { receipt: stored } = &mut mutation.events[0]
    else {
        panic!()
    };
    stored.provider_plan_sha256 = "f".repeat(64);
    assert!(validate_lifecycle_mutation(&before, &[], &mutation).is_err());
}
