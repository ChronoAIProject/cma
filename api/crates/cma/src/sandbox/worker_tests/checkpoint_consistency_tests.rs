use super::*;
use layer2_core::sandbox::store::AgentStore;

#[tokio::test]
async fn failed_conversation_capture_releases_the_prior_checkpoint_writer_hold() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    let agent = lifecycle_queue_tests::ready_agent(&active, now);
    // The durable Agent exists, but its runtime conversation is absent.
    harness.store.create_agent(agent).await.unwrap();
    let pause_at = now + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(pause_at),
            None,
        ))
        .await
        .unwrap();
    harness.run(&active.id, pause_at).await;

    let requests = harness.backend.requests().await;
    let hold = requests
        .iter()
        .position(|(_, request)| matches!(request, RequestFrame::CheckpointForTeardown(_)))
        .unwrap();
    let capture = requests
        .iter()
        .position(|(_, request)| matches!(request, RequestFrame::CaptureAgent(_)))
        .unwrap();
    let resume = requests
        .iter()
        .position(|(_, request)| matches!(request, RequestFrame::ResumeCheckpointWriters(_)))
        .unwrap();
    assert!(hold < capture && capture < resume);
    assert!(
        !harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held
    );
    let failed = harness.stored(&active.id).await;
    assert!(failed.lifecycle.checkpoints.latest.is_none());
    assert_eq!(failed.status, SandboxStatus::Failed);
    let failure = failed.failure.as_ref().unwrap();
    let events = harness
        .store
        .sandbox_events(&active.id, 0, 200)
        .await
        .unwrap();
    assert!(
        matches!(&events.last().unwrap().kind, SandboxEventKind::WorkStepFailed {
        failure: event_failure, ..
    } if event_failure == failure)
    );
}
