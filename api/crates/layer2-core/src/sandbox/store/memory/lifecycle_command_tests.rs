use std::sync::Arc;

use crate::sandbox::lifecycle_test_support::{checkpoint, sandbox, NOW};
use crate::sandbox::store::{
    SandboxEventStore, SandboxLifecycleCommand, SandboxLifecycleCommandDisposition,
    SandboxLifecycleCommandResult, SandboxLifecycleCommandStore, SandboxStore, SandboxStoreError,
};
use crate::sandbox::{
    SandboxEventKind, SandboxLifecycleOperation, SandboxPausePhase, SandboxReviveCause,
    SandboxStatus, SandboxWorkStep,
};
use crate::SubjectId;

use super::InMemorySandboxStore;

#[tokio::test]
async fn concurrent_pause_cancellation_preserves_authority_and_fences_stale_publication() {
    for phase in [
        SandboxPausePhase::CaptureAndPublishCheckpoint,
        SandboxPausePhase::PauseAgents,
    ] {
        let store = Arc::new(InMemorySandboxStore::default());
        let mut value = stored_sandbox();
        value.begin_pause(NOW.into(), None).unwrap();
        value.revision = 1;
        if let Some(SandboxLifecycleOperation::Pause {
            phase: current,
            checkpoint_epoch,
            ..
        }) = value.lifecycle.operation.as_mut()
        {
            *current = phase;
            if phase == SandboxPausePhase::PauseAgents {
                *checkpoint_epoch = Some(1);
                value.lifecycle.checkpoints.latest = Some(checkpoint(1, NOW));
            }
        }
        store.create_sandbox(value.clone()).await.unwrap();
        let authority = value
            .lifecycle
            .operation
            .as_ref()
            .unwrap()
            .authority()
            .clone();
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let store = store.clone();
            let value = value.clone();
            tasks.push(tokio::spawn(async move {
                store
                    .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
                        value.id,
                        value.owner,
                        NOW.into(),
                        SandboxReviveCause::TriggerLaunch,
                    ))
                    .await
                    .unwrap()
            }));
        }
        let mut scheduled = 0;
        for task in tasks {
            let SandboxLifecycleCommandResult::Accepted(outcome) = task.await.unwrap() else {
                panic!("pause cancellation was refused");
            };
            scheduled +=
                usize::from(outcome.disposition == SandboxLifecycleCommandDisposition::Scheduled);
            assert_eq!(outcome.sandbox.status, SandboxStatus::Pausing);
            assert_eq!(
                outcome
                    .sandbox
                    .lifecycle
                    .operation
                    .as_ref()
                    .unwrap()
                    .authority(),
                &authority
            );
            assert!(matches!(
                outcome.sandbox.lifecycle.operation,
                Some(SandboxLifecycleOperation::Pause {
                    phase: SandboxPausePhase::ResumeWriters,
                    ..
                })
            ));
        }
        assert_eq!(scheduled, 1);
        let stored = store.get_sandbox(&value.id).await.unwrap().unwrap();
        assert_eq!(stored.revision, value.revision + 1);
        let mut stale = value.clone();
        stale.revision += 1;
        assert_eq!(
            store.save_sandbox(value.revision, stale).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(store.get_sandbox(&value.id).await.unwrap(), Some(stored));
    }
}

#[tokio::test]
async fn teardown_admission_and_foreign_ownership_refuse_pause_cancellation() {
    for phase in [
        SandboxPausePhase::TeardownRuntime,
        SandboxPausePhase::PublishPaused,
    ] {
        let store = InMemorySandboxStore::default();
        let mut value = stored_sandbox();
        value.begin_pause(NOW.into(), None).unwrap();
        value.revision = 1;
        value.lifecycle.checkpoints.latest = Some(checkpoint(1, NOW));
        if let Some(SandboxLifecycleOperation::Pause {
            phase: current,
            checkpoint_epoch,
            ..
        }) = value.lifecycle.operation.as_mut()
        {
            *current = phase;
            *checkpoint_epoch = Some(1);
        }
        if phase == SandboxPausePhase::PublishPaused {
            value.runtime = None;
        }
        store.create_sandbox(value.clone()).await.unwrap();
        assert_eq!(
            store
                .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
                    value.id.clone(),
                    SubjectId("stranger".into()),
                    NOW.into(),
                    SandboxReviveCause::TriggerLaunch
                ))
                .await,
            Err(SandboxStoreError::NotFound)
        );
        assert_eq!(
            store
                .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
                    value.id.clone(),
                    value.owner.clone(),
                    NOW.into(),
                    SandboxReviveCause::TriggerLaunch
                ))
                .await
                .unwrap(),
            SandboxLifecycleCommandResult::NotEligible(value.clone())
        );
        assert_eq!(
            store.get_sandbox(&value.id).await.unwrap(),
            Some(value.clone())
        );
        assert!(store
            .sandbox_events(&value.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn pause_binds_owner_status_revision_work_and_event_in_one_application() {
    let store = InMemorySandboxStore::default();
    let value = stored_sandbox();
    let expected_revision = value.revision;
    store.create_sandbox(value.clone()).await.unwrap();

    let result = store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            value.id.clone(),
            value.owner.clone(),
            NOW.into(),
            Some("Paused by policy.".into()),
        ))
        .await
        .unwrap();
    let SandboxLifecycleCommandResult::Accepted(outcome) = result else {
        panic!("eligible pause was refused");
    };
    assert_eq!(
        outcome.disposition,
        SandboxLifecycleCommandDisposition::Scheduled
    );
    assert_eq!(outcome.sandbox.status, SandboxStatus::Pausing);
    assert_eq!(outcome.sandbox.revision, expected_revision + 1);
    assert_eq!(
        outcome.sandbox.work.step,
        SandboxWorkStep::RunLifecycleOperation
    );
    assert!(matches!(
        outcome.sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Pause { .. })
    ));
    assert_eq!(
        store.sandbox_events(&value.id, 0, 10).await.unwrap()[0].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Pausing,
        }
    );
}

#[tokio::test]
async fn repeated_and_concurrent_pause_commands_mutate_exactly_once() {
    let store = Arc::new(InMemorySandboxStore::default());
    let value = stored_sandbox();
    store.create_sandbox(value.clone()).await.unwrap();

    let mut tasks = Vec::new();
    for _ in 0..16 {
        let store = store.clone();
        let value = value.clone();
        tasks.push(tokio::spawn(async move {
            store
                .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                    value.id,
                    value.owner,
                    NOW.into(),
                    None,
                ))
                .await
                .unwrap()
        }));
    }
    let mut scheduled = 0;
    let mut replayed = 0;
    for task in tasks {
        let SandboxLifecycleCommandResult::Accepted(outcome) = task.await.unwrap() else {
            panic!("identical pause was refused");
        };
        match outcome.disposition {
            SandboxLifecycleCommandDisposition::Scheduled => scheduled += 1,
            SandboxLifecycleCommandDisposition::AlreadyScheduled => replayed += 1,
            SandboxLifecycleCommandDisposition::AlreadySatisfied => {
                panic!("worker did not complete the pause")
            }
        }
    }
    assert_eq!(scheduled, 1);
    assert_eq!(replayed, 15);
    let stored = store.get_sandbox(&value.id).await.unwrap().unwrap();
    assert_eq!(stored.revision, value.revision + 1);
    assert_eq!(stored.lifecycle.next_authority, 2);
    assert_eq!(
        store.sandbox_events(&value.id, 0, 10).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn pause_hides_foreign_ownership_and_never_overwrites_other_work() {
    let store = InMemorySandboxStore::default();
    let mut busy = stored_sandbox();
    busy.work
        .schedule(SandboxWorkStep::MintRotatedCredential, NOW.into());
    store.create_sandbox(busy.clone()).await.unwrap();

    let foreign = store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            busy.id.clone(),
            SubjectId("stranger".into()),
            NOW.into(),
            None,
        ))
        .await;
    assert_eq!(foreign, Err(SandboxStoreError::NotFound));

    let result = store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            busy.id.clone(),
            busy.owner.clone(),
            NOW.into(),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        result,
        SandboxLifecycleCommandResult::NotEligible(busy.clone())
    );
    assert_eq!(store.get_sandbox(&busy.id).await.unwrap(), Some(busy));
    assert!(store
        .sandbox_events(&sandbox().id, 0, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn credential_rotation_and_retirement_take_precedence_over_pause() {
    for (index, status, step) in [
        (
            0,
            SandboxStatus::CredentialRotating,
            SandboxWorkStep::MintRotatedCredential,
        ),
        (1, SandboxStatus::Retiring, SandboxWorkStep::StopAgents),
    ] {
        let store = InMemorySandboxStore::default();
        let mut value = stored_sandbox();
        value.id.0 = format!("sbx_precedence_{index}");
        value.status = status;
        value.work.schedule(step, NOW.into());
        store.create_sandbox(value.clone()).await.unwrap();

        let result = store
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                value.id.clone(),
                value.owner.clone(),
                NOW.into(),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(
            result,
            SandboxLifecycleCommandResult::NotEligible(value.clone())
        );
        assert_eq!(store.get_sandbox(&value.id).await.unwrap(), Some(value));
    }
}

#[tokio::test]
async fn revive_pins_checkpoint_and_replays_without_changing_its_cause() {
    let store = InMemorySandboxStore::default();
    let mut value = stored_sandbox();
    value.status = SandboxStatus::Paused;
    value.runtime = None;
    value.lifecycle.checkpoints.latest = Some(checkpoint(9, "2026-09-05T00:29:00Z"));
    store.create_sandbox(value.clone()).await.unwrap();

    let first = store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            value.id.clone(),
            value.owner.clone(),
            NOW.into(),
            SandboxReviveCause::AgentMessage,
        ))
        .await
        .unwrap();
    let SandboxLifecycleCommandResult::Accepted(first) = first else {
        panic!("eligible revive was refused");
    };
    assert_eq!(
        first.disposition,
        SandboxLifecycleCommandDisposition::Scheduled
    );
    let Some(SandboxLifecycleOperation::Revive {
        cause,
        checkpoint_epoch,
        ..
    }) = first.sandbox.lifecycle.operation.as_ref()
    else {
        panic!("revive operation was not persisted");
    };
    assert_eq!(*cause, SandboxReviveCause::AgentMessage);
    assert_eq!(*checkpoint_epoch, Some(9));

    let replay = store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            value.id.clone(),
            value.owner.clone(),
            "2026-09-05T00:31:00Z".into(),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    let SandboxLifecycleCommandResult::Accepted(replay) = replay else {
        panic!("revive replay was refused");
    };
    assert_eq!(
        replay.disposition,
        SandboxLifecycleCommandDisposition::AlreadyScheduled
    );
    assert_eq!(replay.sandbox, first.sandbox);
    assert_eq!(
        store.sandbox_events(&value.id, 0, 10).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn already_reached_goal_is_a_content_free_noop() {
    let pause_store = InMemorySandboxStore::default();
    let mut paused = stored_sandbox();
    paused.status = SandboxStatus::Paused;
    paused.runtime = None;
    paused.lifecycle.checkpoints.latest = Some(checkpoint(4, "2026-09-05T00:29:00Z"));
    pause_store.create_sandbox(paused.clone()).await.unwrap();
    let SandboxLifecycleCommandResult::Accepted(outcome) = pause_store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            paused.id.clone(),
            paused.owner.clone(),
            NOW.into(),
            None,
        ))
        .await
        .unwrap()
    else {
        panic!("paused sandbox did not replay pause");
    };
    assert_eq!(
        outcome.disposition,
        SandboxLifecycleCommandDisposition::AlreadySatisfied
    );

    let revive_store = InMemorySandboxStore::default();
    let active = stored_sandbox();
    revive_store.create_sandbox(active.clone()).await.unwrap();
    let SandboxLifecycleCommandResult::Accepted(outcome) = revive_store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            active.id.clone(),
            active.owner.clone(),
            NOW.into(),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap()
    else {
        panic!("active sandbox did not replay revive");
    };
    assert_eq!(
        outcome.disposition,
        SandboxLifecycleCommandDisposition::AlreadySatisfied
    );
}

#[tokio::test]
async fn model_rejections_return_the_exact_stored_aggregate() {
    let pause_store = InMemorySandboxStore::default();
    let mut active = stored_sandbox();
    active.revision = u64::MAX;
    pause_store
        .lock()
        .sandboxes
        .insert(active.id.clone(), active.clone());

    let pause = pause_store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            NOW.into(),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        pause,
        SandboxLifecycleCommandResult::NotEligible(active.clone())
    );
    assert_eq!(
        pause_store.get_sandbox(&active.id).await.unwrap(),
        Some(active)
    );

    let revive_store = InMemorySandboxStore::default();
    let mut paused = stored_sandbox();
    paused.id.0 = "sbx_overflowing_revive".into();
    paused.status = SandboxStatus::Paused;
    paused.runtime = None;
    paused.lifecycle.checkpoints.latest = Some(checkpoint(3, "2026-09-05T00:29:00Z"));
    paused.revision = u64::MAX;
    revive_store
        .lock()
        .sandboxes
        .insert(paused.id.clone(), paused.clone());

    let revive = revive_store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            NOW.into(),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    assert_eq!(
        revive,
        SandboxLifecycleCommandResult::NotEligible(paused.clone())
    );
    assert_eq!(
        revive_store.get_sandbox(&paused.id).await.unwrap(),
        Some(paused)
    );
}

fn stored_sandbox() -> crate::sandbox::Sandbox {
    let mut value = sandbox();
    value.revision = 1;
    value
}

#[tokio::test]
async fn funding_pause_fence_cannot_mutate_a_newer_sandbox_revision() {
    let store = InMemorySandboxStore::default();
    let value = stored_sandbox();
    store.create_sandbox(value.clone()).await.unwrap();
    let mut changed = value.clone();
    changed.name = "newer authority".into();
    changed.revision += 1;
    store
        .save_sandbox(value.revision, changed.clone())
        .await
        .unwrap();
    let command = SandboxLifecycleCommand::pause(
        value.id.clone(),
        value.owner,
        NOW.into(),
        Some("funding unavailable".into()),
    )
    .at_revision(value.revision);
    assert_eq!(
        store.schedule_sandbox_lifecycle(command.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&value.id).await.unwrap().unwrap(),
        changed
    );
    assert!(matches!(
        store
            .schedule_sandbox_lifecycle(command.at_revision(changed.revision))
            .await
            .unwrap(),
        SandboxLifecycleCommandResult::Accepted(_)
    ));
}
