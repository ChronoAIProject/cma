use super::lifecycle_test_support::{agent, checkpoint, sandbox, NOW};
use super::*;

#[test]
fn pause_and_image_rotation_phase_orders_cover_each_restart_boundary() {
    let mut pause = Vec::new();
    let mut phase = Some(SandboxPausePhase::first());
    while let Some(current) = phase {
        pause.push(current);
        phase = current.next();
    }
    assert_eq!(
        pause,
        [
            SandboxPausePhase::CaptureAndPublishCheckpoint,
            SandboxPausePhase::PauseAgents,
            SandboxPausePhase::TeardownRuntime,
            SandboxPausePhase::PublishPaused,
        ]
    );

    let mut rotation = Vec::new();
    let mut phase = Some(SandboxImageRotationPhase::first());
    while let Some(current) = phase {
        rotation.push(current);
        phase = current.next();
    }
    assert_eq!(
        rotation,
        [
            SandboxImageRotationPhase::CaptureAndPublishCheckpoint,
            SandboxImageRotationPhase::PauseAgents,
            SandboxImageRotationPhase::TeardownRuntime,
            SandboxImageRotationPhase::CheckpointVerification,
            SandboxImageRotationPhase::RuntimeProvisioning,
            SandboxImageRotationPhase::CredentialDelivery,
            SandboxImageRotationPhase::HomeRestore,
            SandboxImageRotationPhase::ConversationReopen,
            SandboxImageRotationPhase::QueuedMessageDelivery,
            SandboxImageRotationPhase::PublishActive,
        ]
    );
}

#[test]
fn pause_persists_one_stable_writer_resume_authority_before_work_is_claimed() {
    let mut value = sandbox();
    let notice = inactivity_pause_notice(15);
    let expected_revision = value.revision;

    value.begin_pause(NOW.into(), Some(notice.clone())).unwrap();

    assert_eq!(value.status, SandboxStatus::Pausing);
    assert_eq!(value.revision, expected_revision + 1);
    assert_eq!(value.work.step, SandboxWorkStep::RunLifecycleOperation);
    assert!(value.work.lease.is_none());
    assert_eq!(value.lifecycle.next_authority, 2);
    assert_eq!(
        value.lifecycle.paused_notice.as_deref(),
        Some(notice.as_str())
    );
    let Some(SandboxLifecycleOperation::Pause {
        authority,
        phase,
        checkpoint_epoch,
    }) = value.lifecycle.operation.as_ref()
    else {
        panic!("pause operation was not persisted");
    };
    assert_eq!(authority.generation, 1);
    assert_eq!(authority.requested_at, NOW);
    assert_eq!(
        authority.operation_id,
        "sandbox-lifecycle-v1:sbx_lifecycle:1"
    );
    assert_eq!(
        authority.writer_resume_token.as_ref().map(String::len),
        Some(64)
    );
    assert_eq!(*phase, SandboxPausePhase::CaptureAndPublishCheckpoint);
    assert_eq!(*checkpoint_epoch, None);
    value.validate().unwrap();

    let round_trip: Sandbox =
        serde_json::from_value(serde_json::to_value(&value).unwrap()).unwrap();
    assert_eq!(round_trip, value);
}

#[test]
fn revive_pins_the_latest_checkpoint_and_never_allocates_a_writer_token() {
    let mut value = paused_sandbox();
    value.lifecycle.paused_notice = Some(inactivity_pause_notice(15));

    value
        .begin_revive(SandboxReviveCause::ExplicitRequest, NOW.into())
        .unwrap();

    assert_eq!(value.status, SandboxStatus::Reviving);
    assert_eq!(value.work.step, SandboxWorkStep::RunLifecycleOperation);
    let Some(SandboxLifecycleOperation::Revive {
        authority,
        phase,
        cause,
        checkpoint_epoch,
    }) = value.lifecycle.operation.as_ref()
    else {
        panic!("revive operation was not persisted");
    };
    assert_eq!(authority.generation, 1);
    assert!(authority.writer_resume_token.is_none());
    assert_eq!(*phase, SandboxRevivePhase::CheckpointVerification);
    assert_eq!(*cause, SandboxReviveCause::ExplicitRequest);
    assert_eq!(*checkpoint_epoch, 7);
    value.validate().unwrap();
}

#[test]
fn image_rotation_has_distinct_status_and_preserves_target_identity() {
    let mut value = sandbox();
    let agents = [
        agent("agt_a", "conversation-a", 4),
        agent("agt_b", "conversation-b", 7),
    ];
    let identity = SandboxRotationIdentity::capture(&value, &agents).unwrap();

    value
        .begin_image_rotation("image-v2".into(), identity.clone(), NOW.into())
        .unwrap();

    assert_eq!(value.status, SandboxStatus::Rotating);
    assert_ne!(value.status, SandboxStatus::CredentialRotating);
    let Some(SandboxLifecycleOperation::ImageRotation {
        authority,
        phase,
        target_runtime_version,
        identity: stored_identity,
        ..
    }) = value.lifecycle.operation.as_ref()
    else {
        panic!("image rotation operation was not persisted");
    };
    assert_eq!(*phase, SandboxImageRotationPhase::first());
    assert_eq!(target_runtime_version, "image-v2");
    assert_eq!(stored_identity, &identity);
    assert_eq!(
        authority.writer_resume_token.as_ref().map(String::len),
        Some(64)
    );
    value.validate().unwrap();
}

#[test]
fn lifecycle_validation_rejects_missing_checkpoints_and_tampered_authority() {
    let mut paused = sandbox();
    paused.status = SandboxStatus::Paused;
    paused.runtime = None;
    assert!(paused.validate().is_err());

    let mut pausing = sandbox();
    pausing.begin_pause(NOW.into(), None).unwrap();
    let Some(SandboxLifecycleOperation::Pause { authority, .. }) =
        pausing.lifecycle.operation.as_mut()
    else {
        panic!("pause operation was not persisted");
    };
    authority.writer_resume_token = Some("0".repeat(64));
    assert!(pausing.validate().is_err());
}

#[test]
fn retirement_preempts_and_fences_an_in_progress_lifecycle_operation() {
    let mut value = sandbox();
    value.begin_pause(NOW.into(), None).unwrap();
    let prior_next_authority = value.lifecycle.next_authority;

    value
        .begin_retirement("2026-09-05T00:31:00Z".into())
        .unwrap();

    assert_eq!(value.status, SandboxStatus::Retiring);
    assert_eq!(value.work.step, SandboxWorkStep::StopAgents);
    assert!(value.lifecycle.operation.is_none());
    assert!(value.lifecycle.paused_notice.is_none());
    assert_eq!(value.lifecycle.next_authority, prior_next_authority);
    value.validate().unwrap();
}

#[test]
fn lifecycle_begin_failures_are_atomic_at_revision_overflow() {
    let mut pausing = sandbox();
    pausing.revision = u64::MAX;
    let original = pausing.clone();
    assert!(pausing.begin_pause(NOW.into(), None).is_err());
    assert_eq!(pausing, original);

    let mut reviving = paused_sandbox();
    reviving.revision = u64::MAX;
    let original = reviving.clone();
    assert!(reviving
        .begin_revive(SandboxReviveCause::ExplicitRequest, NOW.into())
        .is_err());
    assert_eq!(reviving, original);

    let mut rotating = sandbox();
    let identity =
        SandboxRotationIdentity::capture(&rotating, &[agent("agt_a", "conversation-a", 4)])
            .unwrap();
    rotating.revision = u64::MAX;
    let original = rotating.clone();
    assert!(rotating
        .begin_image_rotation("image-v2".into(), identity, NOW.into())
        .is_err());
    assert_eq!(rotating, original);

    let mut retiring = sandbox();
    retiring.begin_pause(NOW.into(), None).unwrap();
    retiring.revision = u64::MAX;
    let original = retiring.clone();
    assert!(retiring
        .begin_retirement("2026-09-05T00:31:00Z".into())
        .is_err());
    assert_eq!(retiring, original);
}

#[test]
fn malformed_pause_notice_is_rejected_before_authority_allocation() {
    let mut value = sandbox();
    let original = value.clone();

    assert!(value
        .begin_pause(NOW.into(), Some("x".repeat(513)))
        .is_err());
    assert_eq!(value, original);
}

fn paused_sandbox() -> Sandbox {
    let mut value = sandbox();
    value.status = SandboxStatus::Paused;
    value.runtime = None;
    value.lifecycle.checkpoints.latest = Some(checkpoint(7, "2026-09-05T00:29:00Z"));
    value
}
