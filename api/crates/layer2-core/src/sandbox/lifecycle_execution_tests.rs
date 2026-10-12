use super::lifecycle_test_support::{agent, checkpoint, sandbox, NOW};
use super::*;
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

const HOLDER: &str = "lifecycle-worker";
const LEASE_END: &str = "2026-09-05T01:00:00Z";
const LATER: &str = "2026-09-05T00:31:00Z";

#[test]
fn retained_home_pause_skips_checkpoint_capture_and_keeps_pv_recovery_authority() {
    let mut value = sandbox();
    let mut home = SandboxHome::reserve(
        &value.id,
        HomeVolumePolicy {
            storage_class: "storage".into(),
            capacity_gib: 64,
            template_name: "empty".into(),
            template_generation: "seed-v1".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "sandbox-home".into(),
        storage_generation: "pvc-generation".into(),
    })
    .unwrap();
    value.lifecycle.home = Some(Box::new(home));
    value.lifecycle.runtime_generation_high_water = 1;
    value.runtime.as_mut().unwrap().provision_generation = Some(1);
    value.validate().unwrap();

    value.begin_pause(NOW.into(), None).unwrap();
    assert!(value.retained_home_recovery().is_some());
    claim(&mut value);
    value
        .complete_lifecycle_persistent_pause(HOLDER, fence(&value), LATER.into())
        .unwrap();
    assert_phase(&value, SandboxPausePhase::PauseAgents);
    assert!(value.lifecycle.checkpoints.latest.is_none());

    claim_and_complete(&mut value, |value, fence| {
        value.complete_lifecycle_agent_pause(HOLDER, fence, LATER.into())
    });
    claim_and_complete(&mut value, |value, fence| {
        value.complete_lifecycle_runtime_teardown(HOLDER, fence, LATER.into())
    });
    claim_and_complete(&mut value, |value, fence| {
        value.publish_lifecycle_paused(HOLDER, fence, LATER.into())
    });

    assert_eq!(value.status, SandboxStatus::Paused);
    assert!(value.runtime.is_none());
    assert!(value.lifecycle.checkpoints.latest.is_none());
    assert!(value.retained_home_recovery().is_some());
    value.validate().unwrap();
}

#[test]
fn pause_transitions_publish_one_revision_and_release_each_claim() {
    let mut value = sandbox();
    value
        .begin_pause(NOW.into(), Some("Sandbox paused.".into()))
        .unwrap();
    claim(&mut value);
    assert_eq!(value.next_lifecycle_checkpoint_epoch().unwrap(), 1);

    let captured = checkpoint(1, NOW);
    let revision = value.revision;
    value
        .publish_lifecycle_checkpoint(captured.clone(), HOLDER, fence(&value), LATER.into())
        .unwrap();
    assert_phase(&value, SandboxPausePhase::PauseAgents);
    assert_published(&value, revision);
    assert_eq!(value.lifecycle.checkpoints.latest, Some(captured));

    claim(&mut value);
    let revision = value.revision;
    value
        .complete_lifecycle_agent_pause(HOLDER, fence(&value), LATER.into())
        .unwrap();
    assert_phase(&value, SandboxPausePhase::TeardownRuntime);
    assert_published(&value, revision);

    claim(&mut value);
    let revision = value.revision;
    value
        .complete_lifecycle_runtime_teardown(HOLDER, fence(&value), LATER.into())
        .unwrap();
    assert_phase(&value, SandboxPausePhase::PublishPaused);
    assert!(value.runtime.is_none());
    assert_published(&value, revision);

    claim(&mut value);
    let revision = value.revision;
    value
        .publish_lifecycle_paused(HOLDER, fence(&value), LATER.into())
        .unwrap();
    assert_eq!(value.status, SandboxStatus::Paused);
    assert!(value.lifecycle.operation.is_none());
    assert_eq!(value.work.step, SandboxWorkStep::None);
    assert_published(&value, revision);
}

#[test]
fn revive_runs_only_the_restore_phases_and_clears_the_pause_notice() {
    let mut value = paused();
    value.lifecycle.paused_notice = Some("Sandbox paused.".into());
    value
        .begin_revive(SandboxReviveCause::AgentMessage, NOW.into())
        .unwrap();

    claim(&mut value);
    let pinned = value.lifecycle.checkpoints.latest.clone().unwrap();
    value
        .complete_lifecycle_checkpoint_verification(&pinned, HOLDER, fence(&value), LATER.into())
        .unwrap();
    assert_revive_phase(&value, SandboxRevivePhase::RuntimeProvisioning);

    claim_and_complete(&mut value, |value, fence| {
        value
            .reserve_lifecycle_runtime_generation(2, HOLDER, fence, LATER.into())
            .map(|_| ())
    });
    claim(&mut value);
    value
        .bind_lifecycle_runtime(runtime("image-v1"), HOLDER, fence(&value), LATER.into())
        .unwrap();
    assert_revive_phase(&value, SandboxRevivePhase::CredentialDelivery);

    claim_and_complete(&mut value, |value, fence| {
        value.complete_lifecycle_credential_delivery(HOLDER, fence, LATER.into())
    });
    assert_revive_phase(&value, SandboxRevivePhase::HomeRestore);
    claim_and_complete(&mut value, |value, fence| {
        value.complete_lifecycle_home_restore(HOLDER, fence, LATER.into())
    });
    assert_revive_phase(&value, SandboxRevivePhase::ConversationReopen);
    claim_and_complete(&mut value, |value, fence| {
        value.complete_lifecycle_conversation_reopen(HOLDER, fence, LATER.into())
    });
    assert_revive_phase(&value, SandboxRevivePhase::QueuedMessageDelivery);

    claim_and_complete(&mut value, |value, fence| {
        value.publish_lifecycle_queue_progress(HOLDER, fence, LATER.into())
    });
    assert_revive_phase(&value, SandboxRevivePhase::QueuedMessageDelivery);
    claim_and_complete(&mut value, |value, fence| {
        value.complete_lifecycle_queued_message_delivery(HOLDER, fence, LATER.into())
    });
    assert_revive_phase(&value, SandboxRevivePhase::PublishActive);

    claim(&mut value);
    value
        .publish_lifecycle_active(LEASE_END.into(), HOLDER, fence(&value), LATER.into())
        .unwrap();
    assert_eq!(value.status, SandboxStatus::Active);
    assert!(value.lifecycle.operation.is_none());
    assert!(value.lifecycle.paused_notice.is_none());
    assert_eq!(value.work.step, SandboxWorkStep::WaitForCredentialRotation);
    assert!(value.runtime.is_some());
    value.validate().unwrap();
}

#[test]
fn image_rotation_requires_the_target_runtime_and_accounts_once_at_publication() {
    let mut value = sandbox();
    let agents = [agent("agt_a", "conversation-a", 4)];
    let identity = SandboxRotationIdentity::capture(&value, &agents).unwrap();
    value
        .begin_image_rotation("image-v2".into(), identity, NOW.into())
        .unwrap();
    advance_rotation_to_provisioning(&mut value);

    claim_and_complete(&mut value, |value, fence| {
        value
            .reserve_lifecycle_runtime_generation(2, HOLDER, fence, LATER.into())
            .map(|_| ())
    });
    claim(&mut value);
    let original = value.clone();
    assert!(value
        .bind_lifecycle_runtime(runtime("image-wrong"), HOLDER, fence(&value), LATER.into())
        .is_err());
    assert_eq!(value, original);
    value
        .bind_lifecycle_runtime(runtime("image-v2"), HOLDER, fence(&value), LATER.into())
        .unwrap();

    advance_rotation_to_publish(&mut value);
    claim(&mut value);
    value
        .publish_lifecycle_active(LEASE_END.into(), HOLDER, fence(&value), LATER.into())
        .unwrap();
    assert_eq!(value.status, SandboxStatus::Active);
    assert_eq!(value.lifecycle.rotation.rotations, 1);
    assert_eq!(value.lifecycle.rotation.rotated_at.as_deref(), Some(NOW));
}

#[test]
fn wrong_phase_stale_fence_and_revision_overflow_are_atomic() {
    let mut value = sandbox();
    value.begin_pause(NOW.into(), None).unwrap();
    claim(&mut value);
    let original = value.clone();

    assert!(value
        .complete_lifecycle_agent_pause(HOLDER, fence(&value), LATER.into())
        .is_err());
    assert_eq!(value, original);
    assert!(value
        .publish_lifecycle_checkpoint(checkpoint(1, NOW), HOLDER, fence(&value) + 1, LATER.into())
        .is_err());
    assert_eq!(value, original);

    value.revision = u64::MAX;
    let exhausted = value.clone();
    assert!(value
        .publish_lifecycle_checkpoint(checkpoint(1, NOW), HOLDER, fence(&value), LATER.into())
        .is_err());
    assert_eq!(value, exhausted);
}

#[test]
fn lifecycle_retry_keeps_the_exact_phase_and_increments_attempts() {
    let mut value = sandbox();
    value.begin_pause(NOW.into(), None).unwrap();
    claim(&mut value);
    let operation = value.lifecycle.operation.clone();
    let revision = value.revision;

    value
        .retry_lifecycle(LEASE_END.into(), HOLDER, fence(&value), LATER.into())
        .unwrap();

    assert_eq!(value.lifecycle.operation, operation);
    assert_eq!(value.work.step, SandboxWorkStep::RunLifecycleOperation);
    assert_eq!(value.work.attempts, 1);
    assert_eq!(value.work.ready_at, LEASE_END);
    assert_published(&value, revision);
}

#[test]
fn lifecycle_failure_uses_the_phase_class_and_retains_both_checkpoints() {
    let mut value = paused();
    value.lifecycle.checkpoints.previous = Some(checkpoint(6, "2026-09-05T00:20:00Z"));
    value
        .begin_revive(SandboxReviveCause::ExplicitRequest, NOW.into())
        .unwrap();
    claim(&mut value);
    let expected_history = value.lifecycle.checkpoints.clone();

    let failure = value
        .fail_lifecycle(
            "checkpoint verification failed".into(),
            HOLDER,
            fence(&value),
            LATER.into(),
        )
        .unwrap();
    assert_eq!(failure.class, "restore_failed");
    assert_eq!(value.failure, Some(failure));
    assert_eq!(value.status, SandboxStatus::Failed);
    assert_eq!(value.lifecycle.checkpoints, expected_history);
    assert!(value.lifecycle.operation.is_none());
    assert_eq!(value.work.step, SandboxWorkStep::None);
}

#[test]
fn opportunistic_checkpoint_preserves_active_work_and_rejects_epoch_gaps() {
    let mut value = sandbox();
    value.work =
        SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, LEASE_END.into());
    let original = value.clone();

    assert!(value
        .publish_opportunistic_checkpoint(checkpoint(2, NOW), LATER.into())
        .is_err());
    assert_eq!(value, original);
    value
        .publish_opportunistic_checkpoint(checkpoint(1, NOW), LATER.into())
        .unwrap();
    assert_eq!(value.lifecycle.checkpoints.latest.unwrap().epoch, 1);
    assert_eq!(value.work, original.work);
    assert_eq!(value.revision, original.revision + 1);
}

fn paused() -> Sandbox {
    let mut value = sandbox();
    value.status = SandboxStatus::Paused;
    value.runtime = None;
    value.lifecycle.checkpoints.latest = Some(checkpoint(7, NOW));
    value
}

fn runtime(version: &str) -> RuntimeBinding {
    RuntimeBinding {
        boot_id: None,
        runtime_id: "runtime-replacement".into(),
        runtime_version: version.into(),
        provision_generation: Some(2),
        bound_at: LATER.into(),
    }
}

fn claim(value: &mut Sandbox) {
    value
        .work
        .claim(HOLDER.into(), LATER, LEASE_END.into())
        .unwrap();
}

fn fence(value: &Sandbox) -> u64 {
    value.work.lease.as_ref().unwrap().fence
}

fn claim_and_complete(
    value: &mut Sandbox,
    complete: impl FnOnce(&mut Sandbox, u64) -> Result<(), SandboxModelError>,
) {
    claim(value);
    let fence = fence(value);
    complete(value, fence).unwrap();
}

fn assert_published(value: &Sandbox, previous_revision: u64) {
    assert_eq!(value.revision, previous_revision + 1);
    assert!(value.work.lease.is_none());
    assert_eq!(value.updated_at, LATER);
    value.validate().unwrap();
}

fn assert_phase(value: &Sandbox, expected: SandboxPausePhase) {
    assert!(matches!(
        value.lifecycle.operation,
        Some(SandboxLifecycleOperation::Pause { phase, .. }) if phase == expected
    ));
}

fn assert_revive_phase(value: &Sandbox, expected: SandboxRevivePhase) {
    assert!(matches!(
        value.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive { phase, .. }) if phase == expected
    ));
}

fn advance_rotation_to_provisioning(value: &mut Sandbox) {
    claim(value);
    value
        .publish_lifecycle_checkpoint(checkpoint(1, NOW), HOLDER, fence(value), LATER.into())
        .unwrap();
    claim_and_complete(value, |value, fence| {
        value.complete_lifecycle_agent_pause(HOLDER, fence, LATER.into())
    });
    claim_and_complete(value, |value, fence| {
        value.complete_lifecycle_runtime_teardown(HOLDER, fence, LATER.into())
    });
    let checkpoint = value.lifecycle.checkpoints.latest.clone().unwrap();
    claim_and_complete(value, |value, fence| {
        value.complete_lifecycle_checkpoint_verification(&checkpoint, HOLDER, fence, LATER.into())
    });
}

fn advance_rotation_to_publish(value: &mut Sandbox) {
    claim_and_complete(value, |value, fence| {
        value.complete_lifecycle_credential_delivery(HOLDER, fence, LATER.into())
    });
    claim_and_complete(value, |value, fence| {
        value.complete_lifecycle_home_restore(HOLDER, fence, LATER.into())
    });
    claim_and_complete(value, |value, fence| {
        value.complete_lifecycle_conversation_reopen(HOLDER, fence, LATER.into())
    });
    claim_and_complete(value, |value, fence| {
        value.complete_lifecycle_queued_message_delivery(HOLDER, fence, LATER.into())
    });
}
