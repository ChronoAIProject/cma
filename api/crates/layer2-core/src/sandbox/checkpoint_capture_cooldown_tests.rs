use super::*;
use crate::sandbox::lifecycle_test_support::{checkpoint, sandbox, NOW};
use crate::sandbox::store::{
    validate_lifecycle_mutation, SandboxEventStore, SandboxLifecycleAgentScope,
    SandboxLifecycleExecutionStore, SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore,
    SandboxStoreError, SandboxTransaction, SandboxTransactionStore,
};
use crate::sandbox::{
    sandbox_checkpoint_due, InMemorySandboxStore, SandboxEventKind, SandboxRotationIdentity,
};

const RELEASED: &str = "2026-09-05T00:31:00Z";
const BEFORE: &str = "2026-09-05T00:32:59Z";
const BOUNDARY: &str = "2026-09-05T00:33:00Z";
const EXPIRES: &str = "2026-09-05T00:40:00Z";

fn active() -> Sandbox {
    let mut current = sandbox();
    current.runtime.as_mut().unwrap().provision_generation = Some(1);
    current.lifecycle.runtime_generation_high_water = 1;
    current.lifecycle.checkpoints.latest = Some(checkpoint(1, "2026-09-05T00:00:00Z"));
    current
}

fn failed_capture() -> Sandbox {
    let mut current = active();
    current.begin_checkpoint_capture(NOW.into()).unwrap();
    current
        .claim_checkpoint_capture("capture".into(), NOW.into(), EXPIRES.into())
        .unwrap();
    current
        .require_checkpoint_writer_resume("capture", 1, RELEASED.into())
        .unwrap();
    current
        .claim_checkpoint_capture("resume".into(), RELEASED.into(), EXPIRES.into())
        .unwrap();
    current
}

fn completed_capture() -> Sandbox {
    let mut current = failed_capture();
    current
        .complete_checkpoint_writer_resume("resume", 2, RELEASED.into())
        .unwrap();
    current
}

fn mutation(current: &Sandbox, next: Sandbox, at: &str) -> SandboxLifecycleMutation {
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(current),
        sandbox: next,
        agents: vec![],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: at.into(),
        events: vec![SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }],
    }
}

fn forged_early_begin(current: &Sandbox, at: &str) -> SandboxLifecycleMutation {
    let mut next = current.clone();
    next.lifecycle.checkpoint_capture_completed_at = None;
    next.begin_checkpoint_capture(at.into()).unwrap();
    next.lifecycle.checkpoint_capture_completed_at =
        current.lifecycle.checkpoint_capture_completed_at.clone();
    mutation(current, next, at)
}

#[test]
fn failed_capture_completion_survives_restart_and_defers_only_new_periodic_capture() {
    let debt = failed_capture();
    let mut encoded = serde_json::to_value(&debt).unwrap();
    assert!(encoded["lifecycle"]
        .as_object_mut()
        .unwrap()
        .remove("checkpoint_capture_completed_at")
        .is_none());
    let mut recovered: Sandbox = serde_json::from_value(encoded).unwrap();
    assert_eq!(recovered, debt);
    assert!(recovered
        .lifecycle
        .checkpoint_capture_completed_at
        .is_none());
    assert!(recovered
        .complete_checkpoint_writer_resume("resume", 3, RELEASED.into())
        .is_err());
    assert_eq!(recovered, debt);
    recovered
        .complete_checkpoint_writer_resume("resume", 2, RELEASED.into())
        .unwrap();
    assert_eq!(recovered.lifecycle.checkpoints, debt.lifecycle.checkpoints);
    assert_eq!(
        recovered
            .lifecycle
            .checkpoint_capture_completed_at
            .as_deref(),
        Some(RELEASED)
    );
    assert!(recovered.accepts_agent_and_workspace_admission());
    let restored: Sandbox =
        serde_json::from_value(serde_json::to_value(&recovered).unwrap()).unwrap();
    assert_eq!(restored, recovered);
    for at in [RELEASED, BEFORE, "invalid"] {
        let mut next = restored.clone();
        assert!(!sandbox_checkpoint_due(&next, at));
        assert!(next.begin_checkpoint_capture(at.into()).is_err());
        assert_eq!(next, restored);
    }
    assert!(sandbox_checkpoint_due(&restored, BOUNDARY));
    let mut next = restored.clone();
    next.begin_checkpoint_capture(BOUNDARY.into()).unwrap();
    assert_eq!(
        next.lifecycle.checkpoint_capture_completed_at,
        restored.lifecycle.checkpoint_capture_completed_at
    );
}

#[test]
fn legacy_without_a_completed_capture_does_not_invent_a_cooldown() {
    let mut current = active();
    current.updated_at = NOW.into();
    let encoded = serde_json::to_value(&current).unwrap();
    assert!(encoded["lifecycle"]
        .get("checkpoint_capture_completed_at")
        .is_none());
    let mut restored: Sandbox = serde_json::from_value(encoded).unwrap();
    assert!(sandbox_checkpoint_due(&restored, NOW));
    restored.begin_checkpoint_capture(NOW.into()).unwrap();
    assert!(restored.lifecycle.checkpoint_capture_completed_at.is_none());
}

#[test]
fn cooldown_does_not_delay_manual_pause_rotation_retirement_or_runtime_loss_recovery() {
    let current = completed_capture();
    assert!(!current.checkpoint_capture_cooldown_elapsed(RELEASED));
    let mut paused = current.clone();
    paused.begin_pause(RELEASED.into(), None).unwrap();
    let mut rotated = current.clone();
    let identity = SandboxRotationIdentity::capture(&rotated, &[]).unwrap();
    rotated
        .begin_image_rotation("image-v2".into(), identity, RELEASED.into())
        .unwrap();
    let mut retired = current.clone();
    retired.begin_retirement(RELEASED.into()).unwrap();
    let mut recovered = current.clone();
    recovered
        .recover_runtime_loss(&[], false, RELEASED.into())
        .unwrap();
    for next in [paused, rotated, retired, recovered] {
        assert_eq!(
            next.lifecycle.checkpoint_capture_completed_at,
            current.lifecycle.checkpoint_capture_completed_at
        );
    }
}

#[test]
fn cooldown_is_minted_only_by_exact_resume_completion_and_never_backdated() {
    let current = failed_capture();
    for at in ["invalid", "2026-09-05T00:29:59Z", NOW] {
        let mut next = current.clone();
        assert!(next
            .complete_checkpoint_writer_resume("resume", 2, at.into())
            .is_err());
        assert_eq!(next, current);
    }
    let mut retried = current.clone();
    retried
        .retry_checkpoint_capture(BOUNDARY.into(), "resume", 2, RELEASED.into())
        .unwrap();
    assert!(retried.lifecycle.checkpoint_capture_completed_at.is_none());
    retried
        .claim_checkpoint_capture("recovery".into(), BOUNDARY.into(), EXPIRES.into())
        .unwrap();
    retried
        .complete_checkpoint_writer_resume("recovery", 3, BOUNDARY.into())
        .unwrap();
    assert_eq!(
        retried.lifecycle.checkpoint_capture_completed_at.as_deref(),
        Some(BOUNDARY)
    );
    let completed = completed_capture();
    for timestamp in [None, Some(NOW), Some(BOUNDARY), Some("invalid")] {
        let mut forged = completed.clone();
        forged.lifecycle.checkpoint_capture_completed_at = timestamp.map(str::to_owned);
        assert!(completed
            .validate_checkpoint_capture_successor(&forged)
            .is_err());
        let mut forged = completed.clone();
        forged.lifecycle.checkpoint_capture_completed_at = timestamp.map(str::to_owned);
        assert!(
            validate_lifecycle_mutation(&current, &[], &mutation(&current, forged, RELEASED))
                .is_err()
        );
    }
    let mut invalid = completed;
    invalid.lifecycle.checkpoint_capture_completed_at = Some("invalid".into());
    assert!(invalid.validate().is_err());
    let current = active();
    let mut minted = current.clone();
    minted.lifecycle.checkpoint_capture_completed_at = Some(RELEASED.into());
    assert!(current
        .validate_checkpoint_capture_successor(&minted)
        .is_err());
}

#[tokio::test]
async fn memory_cooldown_rejects_forged_begin_and_generic_timestamp_changes_atomically() {
    let store = InMemorySandboxStore::default();
    let mut debt = failed_capture();
    debt.revision = 1;
    store.create_sandbox(debt.clone()).await.unwrap();
    let mut current = debt.clone();
    current
        .complete_checkpoint_writer_resume("resume", 2, RELEASED.into())
        .unwrap();
    store
        .commit_sandbox_lifecycle(mutation(&debt, current.clone(), RELEASED))
        .await
        .unwrap();
    let events = store.sandbox_events(&current.id, 0, 100).await.unwrap();
    for at in [RELEASED, BEFORE] {
        assert_eq!(
            store
                .commit_sandbox_lifecycle(forged_early_begin(&current, at))
                .await,
            Err(SandboxStoreError::Conflict)
        );
    }
    for timestamp in [None, Some(NOW), Some(BOUNDARY)] {
        let mut forged = current.clone();
        forged.lifecycle.checkpoint_capture_completed_at = timestamp.map(str::to_owned);
        forged.revision += 1;
        assert_eq!(
            store.save_sandbox(current.revision, forged.clone()).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            store
                .commit(SandboxTransaction::SaveSandboxAndAppend {
                    command_revival: None,
                    released_runtime: None,
                    expected_revision: current.revision,
                    sandbox: Box::new(forged),
                    event_at: RELEASED.into(),
                    event: SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::RunLifecycleOperation,
                    },
                })
                .await,
            Err(SandboxStoreError::Conflict)
        );
    }
    assert_eq!(
        store.get_sandbox(&current.id).await.unwrap(),
        Some(current.clone())
    );
    assert_eq!(
        store.sandbox_events(&current.id, 0, 100).await.unwrap(),
        events
    );
    let mut next = current.clone();
    next.begin_checkpoint_capture(BOUNDARY.into()).unwrap();
    store
        .commit_sandbox_lifecycle(mutation(&current, next.clone(), BOUNDARY))
        .await
        .unwrap();
    assert_eq!(store.get_sandbox(&current.id).await.unwrap(), Some(next));
}
