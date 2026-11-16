use crate::store::sandbox_mongo_integration_tests::work_lease_deadline;
use std::sync::Arc;

use layer2_core::sandbox::store::{
    SandboxLaunchCommand, SandboxLaunchCommandDisposition, SandboxLaunchCommandResult,
    SandboxLaunchCommandStore,
};
use layer2_core::sandbox::{
    RuntimeBinding, SandboxCheckpointRef, SandboxLifecycleOperation, SandboxReviveCause,
};
use layer3_adapter::{CheckpointReceipt, WriterState};

use super::*;

const BOUND_AT: &str = "2026-09-05T00:00:30Z";

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn repair_is_atomic_concurrent_and_visible_to_normal_work_discovery() {
    let (client, name) = database("launch_repair").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let failed = admit_failed(&store, "atomic", "launch-atomic").await;

    let results = futures_util::future::join_all((0..16).map(|_| {
        let store = store.clone();
        let failed = failed.clone();
        async move {
            store
                .schedule_sandbox_launch_command(SandboxLaunchCommand::repair_service_grant(
                    failed.id,
                    failed.owner,
                    "launch-atomic".into(),
                    MINUTE.into(),
                ))
                .await
        }
    }))
    .await;
    let mut scheduled = 0;
    let mut replayed = 0;
    for result in results {
        let SandboxLaunchCommandResult::Accepted(outcome) = result.unwrap() else {
            panic!("eligible repair was refused");
        };
        match outcome.disposition {
            SandboxLaunchCommandDisposition::Scheduled => scheduled += 1,
            SandboxLaunchCommandDisposition::AlreadyScheduled => replayed += 1,
            SandboxLaunchCommandDisposition::AlreadySatisfied => {
                panic!("no worker activated this sandbox")
            }
        }
    }
    assert_eq!((scheduled, replayed), (1, 15));

    let stored = store.get_sandbox(&failed.id).await.unwrap().unwrap();
    assert_eq!(stored.revision, failed.revision + 1);
    assert_eq!(stored.status, SandboxStatus::Installing);
    assert_eq!(stored.failure, None);
    assert_eq!(stored.runtime, failed.runtime);
    assert_eq!(stored.work.step, SandboxWorkStep::MintInitialCredential);
    let events = store.sandbox_events(&failed.id, 0, 20).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!((events[0].seq, events[1].seq), (1, 2));
    assert_eq!(
        events[1].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Failed,
            to: SandboxStatus::Installing,
        }
    );
    assert_eq!(
        store.due_sandbox_work(MINUTE, 10).await.unwrap(),
        vec![failed.id.clone()]
    );
    let claimed = store
        .claim_sandbox_work(
            &failed.id,
            "launch-repair-worker",
            MINUTE.into(),
            work_lease_deadline(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.step, SandboxWorkStep::MintInitialCredential);
    assert_eq!(claimed.runtime, failed.runtime);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn repair_rejections_do_not_mutate_state_or_append_events() {
    let (client, name) = database("launch_repair_reject").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let failed = admit_failed(&store, "reject", "launch-reject").await;
    let events = store.sandbox_events(&failed.id, 0, 10).await.unwrap();

    assert_eq!(
        store
            .schedule_sandbox_launch_command(SandboxLaunchCommand::repair_service_grant(
                failed.id.clone(),
                SubjectId::new("other"),
                "launch-reject".into(),
                MINUTE.into(),
            ))
            .await,
        Err(SandboxStoreError::NotFound)
    );
    assert_eq!(
        store
            .schedule_sandbox_launch_command(SandboxLaunchCommand::repair_service_grant(
                failed.id.clone(),
                failed.owner.clone(),
                "launch-wrong".into(),
                MINUTE.into(),
            ))
            .await
            .unwrap(),
        SandboxLaunchCommandResult::OriginMismatch
    );

    let invalid = admit_failed(&store, "state", "launch-state").await;
    let mut wrong_state = invalid.clone();
    wrong_state.failure.as_mut().unwrap().detail = "calendar".into();
    wrong_state.revision += 1;
    store
        .save_sandbox(invalid.revision, wrong_state.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .schedule_sandbox_launch_command(SandboxLaunchCommand::repair_service_grant(
                wrong_state.id.clone(),
                wrong_state.owner.clone(),
                "launch-state".into(),
                MINUTE.into(),
            ))
            .await
            .unwrap(),
        SandboxLaunchCommandResult::NotEligible(wrong_state.clone())
    );

    assert_eq!(
        store.get_sandbox(&failed.id).await.unwrap(),
        Some(failed.clone())
    );
    assert_eq!(
        store.sandbox_events(&failed.id, 0, 10).await.unwrap(),
        events
    );
    assert_eq!(
        store.get_sandbox(&wrong_state.id).await.unwrap(),
        Some(wrong_state)
    );
    assert_eq!(
        store
            .sandbox_events(&invalid.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn launch_revive_persists_trigger_cause_and_honors_reuse_origin_mode() {
    let (client, name) = database("launch_revive").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let created = admit_paused(&store, "created", "launch-created").await;
    let SandboxLaunchCommandResult::Accepted(revived) = store
        .schedule_sandbox_launch_command(SandboxLaunchCommand::revive(
            created.id.clone(),
            created.owner.clone(),
            "launch-created".into(),
            MINUTE.into(),
            false,
        ))
        .await
        .unwrap()
    else {
        panic!("created sandbox revive was refused");
    };
    assert_eq!(
        revived.disposition,
        SandboxLaunchCommandDisposition::Scheduled
    );
    assert!(matches!(
        revived.sandbox.lifecycle.operation,
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

    let older = admit_paused(&store, "older", "launch-older").await;
    assert_eq!(
        store
            .schedule_sandbox_launch_command(SandboxLaunchCommand::revive(
                older.id.clone(),
                older.owner.clone(),
                "launch-new".into(),
                MINUTE.into(),
                false,
            ))
            .await
            .unwrap(),
        SandboxLaunchCommandResult::OriginMismatch
    );
    let SandboxLaunchCommandResult::Accepted(reused) = store
        .schedule_sandbox_launch_command(SandboxLaunchCommand::revive(
            older.id.clone(),
            older.owner.clone(),
            "launch-new".into(),
            MINUTE.into(),
            true,
        ))
        .await
        .unwrap()
    else {
        panic!("owned older Trigger origin was not reusable");
    };
    assert_eq!(
        reused.disposition,
        SandboxLaunchCommandDisposition::Scheduled
    );

    let manual = manual_paused("manual");
    store.create_sandbox(manual.clone()).await.unwrap();
    let SandboxLaunchCommandResult::Accepted(reused) = store
        .schedule_sandbox_launch_command(SandboxLaunchCommand::revive(
            manual.id.clone(),
            manual.owner.clone(),
            "launch-manual".into(),
            MINUTE.into(),
            true,
        ))
        .await
        .unwrap()
    else {
        panic!("owned manual origin was not reusable");
    };
    assert_eq!(
        reused.disposition,
        SandboxLaunchCommandDisposition::Scheduled
    );
    client.database(&name).drop().await.unwrap();
}

async fn admit_failed(store: &MongoSandboxStore, suffix: &str, launch_id: &str) -> Sandbox {
    let fresh = fresh_launch_sandbox(suffix, launch_id);
    admit(store, fresh.clone()).await;
    let mut failed = fresh;
    failed.status = SandboxStatus::Failed;
    failed.failure = Some(TypedFailure {
        class: "service_not_granted".into(),
        detail: "github".into(),
    });
    failed.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: format!("runtime-{suffix}"),
        runtime_version: "image-v1".into(),
        provision_generation: Some(1),
        bound_at: BOUND_AT.into(),
    });
    failed.lifecycle.runtime_generation_high_water = 1;
    failed.work = SandboxWorkState::new(SandboxWorkStep::None, MINUTE.into());
    failed.updated_at = MINUTE.into();
    failed.revision = 2;
    store.save_sandbox(1, failed.clone()).await.unwrap();
    failed
}

async fn admit_paused(store: &MongoSandboxStore, suffix: &str, launch_id: &str) -> Sandbox {
    let fresh = fresh_launch_sandbox(suffix, launch_id);
    admit(store, fresh.clone()).await;
    let mut paused = fresh;
    paused.status = SandboxStatus::Paused;
    paused.credential = sandbox("credential-template").credential;
    paused.credential.reaches = paused.profile.content.nyxid_services.clone();
    paused.work = SandboxWorkState::new(SandboxWorkStep::None, MINUTE.into());
    paused.lifecycle.checkpoints.latest = Some(checkpoint(1));
    paused.updated_at = MINUTE.into();
    paused.revision = 2;
    paused.validate().unwrap();
    store.save_sandbox(1, paused.clone()).await.unwrap();
    paused
}

async fn admit(store: &MongoSandboxStore, sandbox: Sandbox) {
    let name = sandbox.name.clone();
    store
        .admit_sandbox(
            SandboxAdmission::new(sandbox, SandboxAdmissionName::Exact(name), 100, NOW.into())
                .unwrap(),
        )
        .await
        .unwrap();
}

fn fresh_launch_sandbox(suffix: &str, launch_id: &str) -> Sandbox {
    let mut value = provisioning_sandbox(&format!("sbx_launch_{suffix}"));
    value.name = format!("launch-{suffix}");
    value.origin = SandboxOrigin::Trigger {
        launch_id: launch_id.into(),
    };
    value.profile.content.nyxid_services = vec![layer2_core::ServiceId::new("github")];
    value.profile.digest = layer2_core::profiles::content_digest(&value.profile.content).unwrap();
    value.credential.reaches = vec![layer2_core::ServiceId::new("github")];
    value
}

fn manual_paused(suffix: &str) -> Sandbox {
    let mut value = sandbox(&format!("sbx_launch_{suffix}"));
    value.name = format!("launch-{suffix}");
    value.status = SandboxStatus::Paused;
    value.runtime = None;
    value.work = SandboxWorkState::new(SandboxWorkStep::None, MINUTE.into());
    value.lifecycle.checkpoints.latest = Some(checkpoint(2));
    value
}

fn checkpoint(epoch: u64) -> SandboxCheckpointRef {
    SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch,
            object_reference: format!("checkpoints/launch/{epoch}"),
            sha256: "b".repeat(64),
            writers: WriterState::Held,
        },
        &[],
        NOW,
    )
    .unwrap()
}
