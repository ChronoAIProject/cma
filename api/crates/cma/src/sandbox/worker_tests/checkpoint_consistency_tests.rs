use super::*;
use layer2_core::sandbox::store::AgentStore;

#[tokio::test]
async fn lifecycle_worker_future_stays_within_the_stack_budget() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let future = harness.worker.run_one(&created.id, start_time(&created));
    let bytes = std::mem::size_of_val(&future);
    assert!(bytes <= 64 * 1024, "worker future occupies {bytes} bytes");
}

#[tokio::test]
async fn retry_after_confirmed_writer_resume_uses_a_new_durable_hold_token() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
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
    let scheduled = harness.stored(&active.id).await;
    let authority = scheduled
        .lifecycle
        .operation
        .as_ref()
        .unwrap()
        .authority()
        .clone();
    harness
        .backend
        .lose_next_response(WireOperation::CheckpointForTeardown);
    harness.run(&active.id, pause_at).await;
    let retry = harness.stored(&active.id).await;
    let renewed = retry.lifecycle.operation.as_ref().unwrap().authority();
    assert_eq!(renewed.operation_id, authority.operation_id);
    assert_ne!(renewed.writer_resume_token, authority.writer_resume_token);
    assert_eq!(renewed.writer_resume_generation, 1);
    let runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    assert!(
        !harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held
    );
    harness
        .run(&active.id, pause_at + Duration::from_secs(5))
        .await;
    let published = harness.stored(&active.id).await;
    assert!(published.lifecycle.checkpoints.latest.is_some());
    let holds = harness
        .backend
        .requests()
        .await
        .into_iter()
        .filter_map(|(_, request)| match request {
            RequestFrame::CheckpointForTeardown(hold) => Some(hold.writer_resume_token),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        holds,
        vec![
            authority.writer_resume_token.unwrap(),
            renewed.writer_resume_token.clone().unwrap()
        ]
    );
}

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
