use std::sync::Arc;

use crate::sandbox::lifecycle_test_support::{checkpoint, sandbox};
use crate::sandbox::store::{
    SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore, SandboxEventStore,
    SandboxLaunchCommand, SandboxLaunchCommandDisposition, SandboxLaunchCommandResult,
    SandboxLaunchCommandStore, SandboxStore, SandboxStoreError, SandboxWorkerStore,
};
use crate::sandbox::{
    RuntimeBinding, Sandbox, SandboxEventKind, SandboxLifecycleOperation, SandboxOrigin,
    SandboxReviveCause, SandboxStatus, SandboxWorkState, SandboxWorkStep, TypedFailure,
};
use crate::{SandboxId, ServiceId, SubjectId};

use super::InMemorySandboxStore;

const CREATED_AT: &str = "2026-09-05T00:00:00Z";
const BOUND_AT: &str = "2026-09-05T00:01:00Z";
const COMMAND_AT: &str = "2026-09-05T00:03:00Z";
const LEASE_END: &str = "2026-09-05T00:04:00Z";

#[tokio::test]
async fn concurrent_repair_is_one_atomic_event_and_exposes_due_mint_work() {
    let store = Arc::new(InMemorySandboxStore::default());
    let failed = admit_failed(&store, "concurrent", "launch-concurrent").await;

    let mut tasks = Vec::new();
    for _ in 0..16 {
        let store = store.clone();
        let failed = failed.clone();
        tasks.push(tokio::spawn(async move {
            store
                .schedule_sandbox_launch_command(SandboxLaunchCommand::repair_service_grant(
                    failed.id,
                    failed.owner,
                    "launch-concurrent".into(),
                    COMMAND_AT.into(),
                ))
                .await
                .unwrap()
        }));
    }
    let mut scheduled = 0;
    let mut replayed = 0;
    for task in tasks {
        let SandboxLaunchCommandResult::Accepted(outcome) = task.await.unwrap() else {
            panic!("identical repair command was refused");
        };
        match outcome.disposition {
            SandboxLaunchCommandDisposition::Scheduled => scheduled += 1,
            SandboxLaunchCommandDisposition::AlreadyScheduled => replayed += 1,
            SandboxLaunchCommandDisposition::AlreadySatisfied => {
                panic!("repair worker did not activate the sandbox")
            }
        }
    }
    assert_eq!((scheduled, replayed), (1, 15));

    let stored = store.get_sandbox(&failed.id).await.unwrap().unwrap();
    assert_eq!(stored.revision, failed.revision + 1);
    assert_eq!(stored.status, SandboxStatus::Installing);
    assert_eq!(stored.work.step, SandboxWorkStep::MintInitialCredential);
    assert_eq!(stored.runtime, failed.runtime);
    let events = store.sandbox_events(&failed.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[1].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Failed,
            to: SandboxStatus::Installing,
        }
    );
    assert_eq!(
        store.due_sandbox_work(COMMAND_AT, 10).await.unwrap(),
        vec![failed.id.clone()]
    );
    let claimed = store
        .claim_sandbox_work(
            &failed.id,
            "sandbox-worker",
            COMMAND_AT.into(),
            LEASE_END.into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.step, SandboxWorkStep::MintInitialCredential);
    assert_eq!(claimed.runtime, failed.runtime);
}

#[tokio::test]
async fn rejected_repair_identity_and_state_leave_aggregate_and_events_unchanged() {
    let store = InMemorySandboxStore::default();
    let failed = admit_failed(&store, "rejected", "launch-rejected").await;
    let before_events = store.sandbox_events(&failed.id, 0, 10).await.unwrap();

    assert_eq!(
        store
            .schedule_sandbox_launch_command(SandboxLaunchCommand::repair_service_grant(
                failed.id.clone(),
                SubjectId::new("other"),
                "launch-rejected".into(),
                COMMAND_AT.into(),
            ))
            .await,
        Err(SandboxStoreError::NotFound)
    );
    assert_eq!(
        store
            .schedule_sandbox_launch_command(SandboxLaunchCommand::repair_service_grant(
                failed.id.clone(),
                failed.owner.clone(),
                "launch-other".into(),
                COMMAND_AT.into(),
            ))
            .await
            .unwrap(),
        SandboxLaunchCommandResult::OriginMismatch
    );

    let ineligible = admit_failed(&store, "ineligible", "launch-ineligible").await;
    let mut wrong_failure = ineligible.clone();
    wrong_failure.failure.as_mut().unwrap().class = "terminal_validation".into();
    wrong_failure.revision += 1;
    store
        .save_sandbox(ineligible.revision, wrong_failure.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .schedule_sandbox_launch_command(SandboxLaunchCommand::repair_service_grant(
                wrong_failure.id.clone(),
                wrong_failure.owner.clone(),
                "launch-ineligible".into(),
                COMMAND_AT.into(),
            ))
            .await
            .unwrap(),
        SandboxLaunchCommandResult::NotEligible(wrong_failure.clone())
    );

    assert_eq!(
        store.get_sandbox(&failed.id).await.unwrap(),
        Some(failed.clone())
    );
    assert_eq!(
        store.sandbox_events(&failed.id, 0, 10).await.unwrap(),
        before_events
    );
    assert_eq!(
        store.get_sandbox(&wrong_failure.id).await.unwrap(),
        Some(wrong_failure)
    );
    assert_eq!(
        store
            .sandbox_events(&ineligible.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn launch_revive_enforces_creation_origin_but_allows_owned_reuse_origins() {
    let store = InMemorySandboxStore::default();
    let created = admit_paused(&store, "created", "launch-created").await;
    let SandboxLaunchCommandResult::Accepted(outcome) = store
        .schedule_sandbox_launch_command(SandboxLaunchCommand::revive(
            created.id.clone(),
            created.owner.clone(),
            "launch-created".into(),
            COMMAND_AT.into(),
            false,
        ))
        .await
        .unwrap()
    else {
        panic!("created sandbox revive was refused");
    };
    assert_eq!(
        outcome.disposition,
        SandboxLaunchCommandDisposition::Scheduled
    );
    assert!(matches!(
        outcome.sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::TriggerLaunch,
            ..
        })
    ));
    assert_eq!(
        store
            .sandbox_events(&created.id, 0, 10)
            .await
            .unwrap()
            .last()
            .unwrap()
            .kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Paused,
            to: SandboxStatus::Reviving,
        }
    );

    let manual = manual_paused("manual");
    store.create_sandbox(manual.clone()).await.unwrap();
    assert_eq!(
        store
            .schedule_sandbox_launch_command(SandboxLaunchCommand::revive(
                manual.id.clone(),
                manual.owner.clone(),
                "launch-new".into(),
                COMMAND_AT.into(),
                false,
            ))
            .await
            .unwrap(),
        SandboxLaunchCommandResult::OriginMismatch
    );
    let SandboxLaunchCommandResult::Accepted(reused) = store
        .schedule_sandbox_launch_command(SandboxLaunchCommand::revive(
            manual.id.clone(),
            manual.owner.clone(),
            "launch-new".into(),
            COMMAND_AT.into(),
            true,
        ))
        .await
        .unwrap()
    else {
        panic!("manual reuse revive was refused");
    };
    assert_eq!(
        reused.disposition,
        SandboxLaunchCommandDisposition::Scheduled
    );

    let older = admit_paused(&store, "older", "launch-older").await;
    let SandboxLaunchCommandResult::Accepted(reused) = store
        .schedule_sandbox_launch_command(SandboxLaunchCommand::revive(
            older.id.clone(),
            older.owner.clone(),
            "launch-newer".into(),
            COMMAND_AT.into(),
            true,
        ))
        .await
        .unwrap()
    else {
        panic!("older Trigger origin reuse was refused");
    };
    assert_eq!(
        reused.disposition,
        SandboxLaunchCommandDisposition::Scheduled
    );
}

async fn admit_failed(store: &InMemorySandboxStore, suffix: &str, launch_id: &str) -> Sandbox {
    let fresh = fresh_launch_sandbox(suffix, launch_id);
    admit(store, fresh.clone()).await;
    let mut failed = fresh;
    failed.status = SandboxStatus::Failed;
    failed.failure = Some(TypedFailure {
        class: "service_not_granted".into(),
        detail: "github".into(),
    });
    failed.runtime = Some(RuntimeBinding {
        runtime_id: format!("runtime-{suffix}"),
        runtime_version: "image-v1".into(),
        provision_generation: None,
        bound_at: BOUND_AT.into(),
    });
    failed.work = SandboxWorkState::new(SandboxWorkStep::None, COMMAND_AT.into());
    failed.updated_at = COMMAND_AT.into();
    failed.revision = 2;
    store.save_sandbox(1, failed.clone()).await.unwrap();
    failed
}

async fn admit_paused(store: &InMemorySandboxStore, suffix: &str, launch_id: &str) -> Sandbox {
    let fresh = fresh_launch_sandbox(suffix, launch_id);
    admit(store, fresh.clone()).await;
    let mut paused = fresh;
    paused.status = SandboxStatus::Paused;
    paused.credential = sandbox().credential;
    paused.work = SandboxWorkState::new(SandboxWorkStep::None, COMMAND_AT.into());
    paused.lifecycle.checkpoints.latest = Some(checkpoint(1, BOUND_AT));
    paused.updated_at = COMMAND_AT.into();
    paused.revision = 2;
    store.save_sandbox(1, paused.clone()).await.unwrap();
    paused
}

async fn admit(store: &InMemorySandboxStore, sandbox: Sandbox) {
    let name = sandbox.name.clone();
    store
        .admit_sandbox(
            SandboxAdmission::new(
                sandbox,
                SandboxAdmissionName::Exact(name),
                100,
                CREATED_AT.into(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
}

fn fresh_launch_sandbox(suffix: &str, launch_id: &str) -> Sandbox {
    let mut value = sandbox();
    value.id = SandboxId(format!("sbx_launch_store_{suffix}"));
    value.name = format!("launch-store-{suffix}");
    value.origin = SandboxOrigin::Trigger {
        launch_id: launch_id.into(),
    };
    value.profile.content.nyxid_services = vec![ServiceId::new("github")];
    value.status = SandboxStatus::Provisioning;
    value.failure = None;
    value.runtime = None;
    value.credential.reaches = vec![ServiceId::new("github")];
    value.credential.key_generation = 0;
    value.credential.delivered = false;
    value.credential.current = None;
    value.credential.pending = None;
    value.credential.revoke_debt.clear();
    value.work = SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, CREATED_AT.into());
    value.lifecycle = Default::default();
    value.last_activity_at = CREATED_AT.into();
    value.created_at = CREATED_AT.into();
    value.updated_at = CREATED_AT.into();
    value.revision = 1;
    value
}

fn manual_paused(suffix: &str) -> Sandbox {
    let mut value = sandbox();
    value.id = SandboxId(format!("sbx_launch_store_{suffix}"));
    value.name = format!("launch-store-{suffix}");
    value.status = SandboxStatus::Paused;
    value.runtime = None;
    value.work = SandboxWorkState::new(SandboxWorkStep::None, COMMAND_AT.into());
    value.lifecycle.checkpoints.latest = Some(checkpoint(2, BOUND_AT));
    value.revision = 1;
    value
}
