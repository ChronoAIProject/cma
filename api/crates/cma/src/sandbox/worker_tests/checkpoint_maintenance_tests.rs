use super::*;
use layer2_core::sandbox::store::{
    AgentStore, SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore,
    SandboxLifecycleMutation, SandboxMutationGuard,
};
use layer2_core::sandbox::{SandboxCheckpointCapturePhase, SANDBOX_CHECKPOINT_INTERVAL_SECONDS};
use layer3_adapter::{CheckpointHoldRequest, CheckpointRequest};

use crate::sandbox::checkpoint_maintenance::SandboxCheckpointMaintenance;

fn maintenance(harness: &Harness, holder: &str) -> Arc<SandboxCheckpointMaintenance> {
    SandboxCheckpointMaintenance::new(
        harness.store.clone(),
        harness.adapter.clone(),
        "test".into(),
        holder.into(),
    )
}

async fn active() -> (Harness, Sandbox, SystemTime) {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    (harness, active, now + Duration::from_secs(1))
}

fn token(sandbox: &Sandbox) -> String {
    sandbox
        .lifecycle
        .checkpoint_capture
        .as_ref()
        .unwrap()
        .authority
        .writer_resume_token
        .clone()
        .unwrap()
}

#[tokio::test]
async fn periodic_capture_persists_authority_before_io_and_retains_debt_until_resume() {
    let (harness, active, now) = active().await;
    let worker = maintenance(&harness, "periodic-a");
    let runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    assert_eq!(worker.run_once(now).await.unwrap(), 1);
    let begun = harness.stored(&active.id).await;
    let authority = token(&begun);
    assert_eq!(begun.work, active.work);
    assert!(!harness
        .backend
        .requests()
        .await
        .iter()
        .any(|(_, request)| { matches!(request, RequestFrame::CheckpointForTeardown(_)) }));
    assert_eq!(worker.run_once(now).await.unwrap(), 1);
    let published = harness.stored(&active.id).await;
    assert_eq!(published.status, SandboxStatus::Active);
    assert!(published.lifecycle.checkpoints.latest.is_some());
    assert_eq!(token(&published), authority);
    assert_eq!(
        published
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .phase,
        SandboxCheckpointCapturePhase::ResumeWriters
    );
    assert!(
        harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held
    );
    assert_eq!(worker.run_once(now).await.unwrap(), 1);
    let resumed = harness.stored(&active.id).await;
    assert!(resumed.lifecycle.checkpoint_capture.is_none());
    assert_eq!(resumed.work, active.work);
    assert!(
        !harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held
    );
    assert_eq!(worker.run_once(now).await.unwrap(), 0);
    assert_eq!(
        worker
            .run_once(now + Duration::from_secs(SANDBOX_CHECKPOINT_INTERVAL_SECONDS))
            .await
            .unwrap(),
        1
    );
    assert_ne!(token(&harness.stored(&active.id).await), authority);
}

#[tokio::test]
async fn failed_conversation_capture_keeps_resume_debt_and_publishes_nothing() {
    let (harness, active, now) = active().await;
    let agent = lifecycle_queue_tests::ready_agent(&active, now);
    harness.store.create_agent(agent).await.unwrap();
    let worker = maintenance(&harness, "periodic-a");
    let runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    worker.run_once(now).await.unwrap();
    let authority = token(&harness.stored(&active.id).await);
    worker.run_once(now).await.unwrap();
    let aborted = harness.stored(&active.id).await;
    assert_eq!(aborted.status, SandboxStatus::Active);
    assert!(aborted.lifecycle.checkpoints.latest.is_none());
    assert_eq!(token(&aborted), authority);
    assert_eq!(
        aborted.lifecycle.checkpoint_capture.as_ref().unwrap().phase,
        SandboxCheckpointCapturePhase::ResumeWriters
    );
    assert!(
        harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held
    );
    worker.run_once(now).await.unwrap();
    assert!(harness
        .stored(&active.id)
        .await
        .lifecycle
        .checkpoint_capture
        .is_none());
    assert!(
        !harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held
    );
}

#[tokio::test]
async fn failed_resume_retains_blocking_debt_until_a_restarted_worker_acknowledges_it() {
    let (harness, active, now) = active().await;
    let worker = maintenance(&harness, "periodic-a");
    worker.run_once(now).await.unwrap();
    worker.run_once(now).await.unwrap();
    let published = harness.stored(&active.id).await;
    let authority = token(&published);
    harness.backend.fail_next(
        WireOperation::ResumeCheckpointWriters,
        "resume_unavailable",
        "writer resume unavailable",
    );
    worker.run_once(now).await.unwrap();
    let retained = harness.stored(&active.id).await;
    assert_eq!(token(&retained), authority);
    assert_eq!(
        retained
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .work
            .attempts,
        1
    );
    assert!(harness
        .store
        .claim_sandbox_work(
            &active.id,
            "retirement-worker",
            retained.work.ready_at.clone(),
            rfc3339_at(now + Duration::from_secs(86_400)),
        )
        .await
        .unwrap()
        .is_none());
    let restarted = maintenance(&harness, "periodic-b");
    assert_eq!(
        restarted
            .run_once(now + Duration::from_secs(29))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        restarted
            .run_once(now + Duration::from_secs(30))
            .await
            .unwrap(),
        1
    );
    let resumed = harness.stored(&active.id).await;
    assert!(resumed.lifecycle.checkpoint_capture.is_none());
    assert_eq!(
        resumed.lifecycle.checkpoints,
        published.lifecycle.checkpoints
    );
}

#[tokio::test]
async fn expired_capture_is_aborted_and_resumed_without_recapture_even_after_retirement() {
    let (harness, active, now) = active().await;
    let worker = maintenance(&harness, "periodic-a");
    worker.run_once(now).await.unwrap();
    let begun = harness.stored(&active.id).await;
    let authority = token(&begun);
    let mut claimed = begun.clone();
    claimed
        .claim_checkpoint_capture(
            "departed-worker".into(),
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(60)),
        )
        .unwrap();
    harness
        .store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&begun),
            sandbox: claimed.clone(),
            agents: Vec::new(),
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: rfc3339_at(now),
            events: vec![SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }],
        })
        .await
        .unwrap();
    let runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    harness
        .adapter
        .checkpoint_for_teardown(
            &runtime,
            &CheckpointHoldRequest {
                checkpoint: CheckpointRequest {
                    epoch: claimed
                        .lifecycle
                        .checkpoint_capture
                        .as_ref()
                        .unwrap()
                        .checkpoint_epoch,
                    exclusions: Vec::new(),
                },
                writer_resume_token: authority.clone(),
            },
        )
        .await
        .unwrap();
    let mut retiring = claimed.clone();
    retiring.begin_retirement(rfc3339_at(now)).unwrap();
    harness
        .store
        .save_sandbox(claimed.revision, retiring)
        .await
        .unwrap();
    let restarted = maintenance(&harness, "periodic-b");
    assert_eq!(
        restarted
            .run_once(now + Duration::from_secs(59))
            .await
            .unwrap(),
        0
    );
    let recovered_at = now + Duration::from_secs(60);
    assert_eq!(restarted.run_once(recovered_at).await.unwrap(), 1);
    assert_eq!(token(&harness.stored(&active.id).await), authority);
    assert_eq!(restarted.run_once(recovered_at).await.unwrap(), 1);
    let resumed = harness.stored(&active.id).await;
    assert_eq!(resumed.status, SandboxStatus::Retiring);
    assert!(resumed.lifecycle.checkpoint_capture.is_none());
    assert!(resumed.lifecycle.checkpoints.latest.is_none());
    assert!(
        !harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held
    );
    assert_eq!(
        harness
            .backend
            .requests()
            .await
            .iter()
            .filter(|(_, request)| { matches!(request, RequestFrame::CheckpointForTeardown(_)) })
            .count(),
        1
    );
}
