use super::*;
use crate::sandbox::lifecycle_test_support::{agent, checkpoint, sandbox, NOW};
use crate::sandbox::store::{
    validate_checkpoint_mutation, validate_lifecycle_mutation, SandboxCheckpointMutation,
    SandboxLifecycleAgentScope, SandboxLifecycleMutation, SandboxMutationGuard,
};
use crate::sandbox::{SandboxEventKind, SandboxHome};
use layer3_adapter::{
    CheckpointReceipt, HomeSnapshotExportConversation, HomeSnapshotExportJobReceipt,
    HomeSnapshotReceipt, HomeSnapshotViewReceipt, HomeVolumePolicy, HomeVolumeReceipt, WriterState,
};

const EXPIRES: &str = "2026-09-05T00:35:01Z";
const AFTER_EXPIRY: &str = "2026-09-05T00:36:01Z";
const HOLDER: &str = "background-worker";

fn active() -> Sandbox {
    let mut value = sandbox();
    value.runtime.as_mut().unwrap().provision_generation = Some(4);
    value.lifecycle.runtime_generation_high_water = 4;
    let mut home = SandboxHome::reserve(
        &value.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "cma-home-v1".into(),
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
        storage_reference: "home-pvc".into(),
        storage_generation: "original-pvc-uid".into(),
    })
    .unwrap();
    value.lifecycle.home = Some(Box::new(home));
    value.validate().unwrap();
    value
}

fn plan(value: &Sandbox) -> SandboxBackgroundCheckpointPlan {
    let epoch = value.next_checkpoint_epoch().unwrap();
    SandboxBackgroundCheckpointPlan {
        snapshot: HomeSnapshotRequest {
            capture_id: format!("{}-checkpoint-{epoch}", value.id),
            epoch,
            home: value.lifecycle.home.as_ref().unwrap().attachment().unwrap(),
            snapshot_class: "cma-home-v1".into(),
        },
        runtime: value.runtime.clone().unwrap(),
        conversations: vec![HomeSnapshotConversationIdentity {
            agent_id: "agt_a".into(),
            conversation_id: "conversation-a".into(),
        }],
        image: format!("registry/runtime@sha256:{}", "a".repeat(64)),
    }
}

fn capture(value: &Sandbox) -> &SandboxBackgroundCheckpoint {
    value.lifecycle.background_checkpoint.as_deref().unwrap()
}

fn mutation(current: &Sandbox, next: Sandbox) -> SandboxLifecycleMutation {
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(current),
        appended_at: next.updated_at.clone(),
        sandbox: next,
        agents: vec![],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        events: vec![SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }],
    }
}

fn begin(value: &mut Sandbox) {
    let current = value.clone();
    value
        .begin_background_checkpoint(plan(value), NOW.into())
        .unwrap();
    validate_lifecycle_mutation(&current, &[], &mutation(&current, value.clone())).unwrap();
}

fn claim(value: &mut Sandbox) -> u64 {
    let current = value.clone();
    value
        .claim_background_checkpoint(HOLDER.into(), NOW.into(), EXPIRES.into())
        .unwrap();
    validate_lifecycle_mutation(&current, &[], &mutation(&current, value.clone())).unwrap();
    capture(value).work.lease.as_ref().unwrap().fence
}

fn advance(value: &mut Sandbox, progress: SandboxBackgroundCheckpointProgress) {
    let fence = claim(value);
    let current = value.clone();
    value
        .advance_background_checkpoint(progress, HOLDER, fence, NOW.into(), NOW.into())
        .unwrap();
    validate_lifecycle_mutation(&current, &[], &mutation(&current, value.clone())).unwrap();
}

fn snapshot_observation(value: &Sandbox, ready: bool) -> HomeSnapshotObservation {
    let request = &capture(value).plan.snapshot;
    HomeSnapshotObservation {
        receipt: HomeSnapshotReceipt {
            capture_id: request.capture_id.clone(),
            request_sha256: request.computed_sha256(),
            snapshot_reference: "snapshot".into(),
            snapshot_generation: "original-snapshot-uid".into(),
        },
        ready,
        captured_at: ready.then(|| NOW.into()),
    }
}

fn uploaded() -> Sandbox {
    let mut value = active();
    begin(&mut value);
    let mut progress = capture(&value).progress.clone();
    progress.snapshot = Some(snapshot_observation(&value, false));
    advance(&mut value, progress.clone());
    // Simulate a restart while CSI is still copying.
    value = serde_json::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap();
    progress.snapshot = Some(snapshot_observation(&value, true));
    advance(&mut value, progress.clone());
    let export = capture(&value).export_request().unwrap();
    let digest = export.computed_sha256().unwrap();
    progress.view = Some(HomeSnapshotViewObservation {
        receipt: HomeSnapshotViewReceipt {
            request_sha256: digest.clone(),
            volume_reference: format!("cma-view-{}", &digest[..52]),
            volume_generation: "view-uid".into(),
        },
        bound_volume: None,
    });
    advance(&mut value, progress.clone());
    // The exporter is admitted while the view still awaits a consumer.
    let job = capture(&value).job_request().unwrap();
    let digest = job.computed_sha256().unwrap();
    progress.job = Some(HomeSnapshotExportJobObservation {
        receipt: HomeSnapshotExportJobReceipt {
            request_sha256: digest.clone(),
            pod_reference: format!("cma-export-{}", &digest[..48]),
            pod_generation: "exporter-uid".into(),
        },
        view: progress.view.clone().unwrap(),
        phase: HomeSnapshotExportJobPhase::Pending,
    });
    advance(&mut value, progress.clone());
    progress.job.as_mut().unwrap().view.bound_volume = Some("derived-pv".into());
    progress.job.as_mut().unwrap().phase = HomeSnapshotExportJobPhase::Ready;
    advance(&mut value, progress.clone());
    progress.uploaded = Some(HomeSnapshotExportReceipt {
        request_sha256: export.computed_sha256().unwrap(),
        archive_bytes: 123,
        checkpoint: CheckpointReceipt {
            epoch: export.snapshot.epoch,
            object_reference: format!("home-{}.cma", "b".repeat(64)),
            sha256: "b".repeat(64),
            writers: WriterState::Resumed,
        },
        conversations: vec![HomeSnapshotExportConversation {
            identity: export.conversations[0].clone(),
            archive_sha256: "c".repeat(64),
        }],
    });
    advance(&mut value, progress);
    value
}

fn sealed(value: &Sandbox) -> SandboxCheckpointRef {
    let capture = capture(value);
    SandboxCheckpointRef::seal_export(
        &capture.export_request().unwrap(),
        capture.progress.uploaded.as_ref().unwrap(),
    )
    .unwrap()
}

#[test]
fn background_checkpoint_keeps_admission_and_pause_available_and_freezes_plan() {
    let mut value = active();
    let work = value.work.clone();
    begin(&mut value);
    assert!(value.accepts_agent_and_workspace_admission());
    assert!(value.pause_eligibility().is_ok());
    assert!(value.lifecycle.checkpoint_capture.is_none());
    assert_eq!(value.work, work);
    let mut forged = value.clone();
    forged
        .lifecycle
        .background_checkpoint
        .as_mut()
        .unwrap()
        .plan
        .conversations
        .clear();
    forged.revision += 1;
    assert!(validate_lifecycle_mutation(&value, &[], &mutation(&value, forged.clone())).is_err());
    assert!(value
        .validate_checkpoint_capture_successor(&forged)
        .is_err());
}

#[test]
fn background_checkpoint_publishes_frozen_evidence_after_live_agent_advances() {
    let mut value = uploaded();
    let fence = claim(&mut value);
    let checkpoint = sealed(&value);
    let current = value.clone();
    value
        .publish_background_checkpoint(checkpoint.clone(), HOLDER, fence, NOW.into())
        .unwrap();
    let mut live = agent("agt_a", "conversation-a", 1000);
    live.conversation.active_turn = Some(999);
    let mutation = SandboxCheckpointMutation {
        guard: SandboxMutationGuard::capture(&current),
        agents: vec![],
        nonblocking: true,
        checkpoint,
        sandbox: value.clone(),
        appended_at: NOW.into(),
    };
    validate_checkpoint_mutation(&current, &[(live, 1000)], &mutation).unwrap();
    assert!(value.accepts_agent_and_workspace_admission());
    assert!(value.lifecycle.checkpoints.previous.is_none());
    let mut forged = mutation.clone();
    forged.nonblocking = false;
    assert!(validate_checkpoint_mutation(&current, &[], &forged).is_err());
    forged = mutation;
    forged.checkpoint.conversations[0].archive_sha256 = "d".repeat(64);
    assert!(validate_checkpoint_mutation(&current, &[], &forged).is_err());
    let fence = claim(&mut value);
    let current = value.clone();
    value
        .complete_background_checkpoint_cleanup(HOLDER, fence, NOW.into())
        .unwrap();
    validate_lifecycle_mutation(
        &current,
        &[],
        &super::tests::mutation(&current, value.clone()),
    )
    .unwrap();
    assert!(value.lifecycle.background_checkpoint.is_none());
    assert_eq!(value.next_checkpoint_epoch().unwrap(), 2);
}

#[test]
fn background_checkpoint_bare_nonblocking_flag_cannot_bypass_live_proof() {
    let current = active();
    let checkpoint = checkpoint(1, NOW);
    let mut next = current.clone();
    next.publish_opportunistic_checkpoint(checkpoint.clone(), NOW.into())
        .unwrap();
    let mutation = SandboxCheckpointMutation {
        guard: SandboxMutationGuard::capture(&current),
        agents: vec![],
        nonblocking: true,
        checkpoint,
        sandbox: next,
        appended_at: NOW.into(),
    };
    assert!(validate_checkpoint_mutation(&current, &[], &mutation).is_err());
}

#[test]
fn background_checkpoint_abandoned_epoch_is_never_reused_and_pause_supersedes_export() {
    let mut value = uploaded();
    let fence = claim(&mut value);
    let old_checkpoint = sealed(&value);
    value.begin_pause(NOW.into(), None).unwrap();
    assert!(value
        .publish_background_checkpoint(old_checkpoint, HOLDER, fence, NOW.into())
        .is_err());
    let foreground_fence = value
        .work
        .claim("pause-worker".into(), NOW, EXPIRES.into())
        .unwrap();
    assert_eq!(value.next_lifecycle_checkpoint_epoch().unwrap(), 2);
    value
        .publish_lifecycle_checkpoint(
            checkpoint(2, NOW),
            "pause-worker",
            foreground_fence,
            NOW.into(),
        )
        .unwrap();
    assert!(!capture(&value).source_is_current(&value));
    let mut progress = capture(&value).progress.clone();
    progress.cancelled = true;
    value
        .advance_background_checkpoint(progress, HOLDER, fence, NOW.into(), NOW.into())
        .unwrap();
    let fence = claim(&mut value);
    value
        .complete_background_checkpoint_cleanup(HOLDER, fence, NOW.into())
        .unwrap();
    assert_eq!(value.next_checkpoint_epoch().unwrap(), 3);
    assert_eq!(
        value.lifecycle.checkpoints.latest.as_ref().unwrap().epoch,
        2
    );
}

#[test]
fn background_checkpoint_restart_preserves_snapshot_uid_and_rejects_expired_workers() {
    let mut value = active();
    begin(&mut value);
    let mut progress = capture(&value).progress.clone();
    progress.snapshot = Some(snapshot_observation(&value, false));
    advance(&mut value, progress.clone());
    let fence = claim(&mut value);
    let before = value.clone();
    assert!(value
        .advance_background_checkpoint(
            progress.clone(),
            HOLDER,
            fence,
            EXPIRES.into(),
            EXPIRES.into()
        )
        .is_err());
    assert_eq!(value, before);
    let mut reopened: Sandbox =
        serde_json::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap();
    reopened
        .claim_background_checkpoint(
            "replacement-worker".into(),
            EXPIRES.into(),
            AFTER_EXPIRY.into(),
        )
        .unwrap();
    assert!(reopened
        .advance_background_checkpoint(
            progress.clone(),
            HOLDER,
            fence,
            EXPIRES.into(),
            EXPIRES.into()
        )
        .is_err());
    let fence = capture(&reopened).work.lease.as_ref().unwrap().fence;
    progress
        .snapshot
        .as_mut()
        .unwrap()
        .receipt
        .snapshot_generation = "replacement-uid".into();
    assert!(reopened
        .advance_background_checkpoint(
            progress,
            "replacement-worker",
            fence,
            EXPIRES.into(),
            EXPIRES.into()
        )
        .is_err());
    assert_eq!(
        capture(&reopened).progress.snapshot,
        capture(&before).progress.snapshot
    );
}

#[test]
fn background_checkpoint_requires_receipts_and_cleanup_terminal_state() {
    let mut value = active();
    begin(&mut value);
    let fence = claim(&mut value);
    assert!(value
        .complete_background_checkpoint_cleanup(HOLDER, fence, NOW.into())
        .is_err());
    let mut progress = capture(&value).progress.clone();
    progress.published = true;
    assert!(value
        .advance_background_checkpoint(progress, HOLDER, fence, NOW.into(), NOW.into())
        .is_err());
    let mut erased = value.clone();
    erased.lifecycle.background_checkpoint = None;
    assert!(value
        .validate_checkpoint_capture_successor(&erased)
        .is_err());
    erased = value.clone();
    erased.lifecycle.checkpoint_epoch_high_water = 0;
    assert!(value
        .validate_checkpoint_capture_successor(&erased)
        .is_err());
}

#[test]
fn background_checkpoint_epoch_overflow_fails_without_mutation() {
    let mut value = active();
    value.lifecycle.checkpoint_epoch_high_water = u64::MAX;
    let before = value.clone();
    assert!(value.next_checkpoint_epoch().is_err());
    assert_eq!(value, before);
}
