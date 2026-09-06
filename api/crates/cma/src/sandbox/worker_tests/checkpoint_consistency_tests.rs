use super::*;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, SandboxLifecycleOperation, SandboxPausePhase,
};

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

#[tokio::test]
async fn lost_hold_response_during_agent_pause_restarts_with_fresh_capture() {
    retry_late_hold(false, false).await;
}

#[tokio::test]
async fn lost_hold_response_during_teardown_recaptures_already_paused_agents() {
    retry_late_hold(true, false).await;
}

#[tokio::test]
async fn restart_after_agent_pause_resume_retires_old_authority_before_recapture() {
    retry_late_hold(false, true).await;
}

#[tokio::test]
async fn restart_after_teardown_resume_retires_old_authority_before_recapture() {
    retry_late_hold(true, true).await;
}

#[tokio::test]
async fn terminal_resume_error_during_agent_pause_retains_writer_debt() {
    unconfirmed_late_resume(false).await;
}

#[tokio::test]
async fn terminal_resume_error_during_teardown_retains_writer_debt() {
    unconfirmed_late_resume(true).await;
}

async fn late_pause(parked: bool) -> (Harness, Sandbox, Agent, SystemTime) {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let agent = lifecycle_recovery_tests::prepare_agent(&harness, &active, now, 1).await;
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
    if parked {
        harness.run(&active.id, pause_at).await;
    }
    let current = harness.stored(&active.id).await;
    assert_pause_phase(
        &current,
        if parked {
            SandboxPausePhase::TeardownRuntime
        } else {
            SandboxPausePhase::PauseAgents
        },
    );
    let agent = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    (harness, current, agent, pause_at)
}

async fn retry_late_hold(parked: bool, resumed_before_restart: bool) {
    let (harness, current, agent, pause_at) = late_pause(parked).await;
    let authority = current.lifecycle.operation.as_ref().unwrap().authority();
    if resumed_before_restart {
        let runtime = RuntimeHandle(current.runtime.as_ref().unwrap().runtime_id.clone());
        harness
            .adapter
            .resume_checkpoint_writers(
                &runtime,
                &layer3_adapter::CheckpointWriterResumeRequest {
                    writer_resume_token: authority.writer_resume_token.clone().unwrap(),
                },
            )
            .await
            .unwrap();
        if !parked {
            harness.backend.fail_next(
                WireOperation::CheckpointForTeardown,
                "checkpoint_hold_retired",
                "checkpoint token was already resumed",
            );
        }
    } else {
        harness
            .backend
            .lose_next_response(WireOperation::CheckpointForTeardown);
    }
    harness.run(&current.id, pause_at).await;
    let retry = harness.stored(&current.id).await;
    assert_pause_phase(&retry, SandboxPausePhase::CaptureAndPublishCheckpoint);
    let renewed = retry.lifecycle.operation.as_ref().unwrap().authority();
    assert_eq!(renewed.operation_id, authority.operation_id);
    assert_eq!(renewed.writer_resume_generation, 1);
    assert_ne!(renewed.writer_resume_token, authority.writer_resume_token);
    assert!(retry
        .lifecycle
        .operation
        .as_ref()
        .unwrap()
        .checkpoint_epoch()
        .is_none());
    assert_eq!(retry.lifecycle.checkpoints, current.lifecycle.checkpoints);
    let runtime = RuntimeHandle(current.runtime.as_ref().unwrap().runtime_id.clone());
    assert!(
        !harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held
    );
    assert_eq!(
        harness.store.get_agent(&agent.id).await.unwrap().unwrap(),
        agent
    );
    let retry_at = pause_at + Duration::from_secs(5);
    harness.run(&current.id, retry_at).await;
    let captured = harness.stored(&current.id).await;
    let checkpoint = captured.lifecycle.checkpoints.latest.as_ref().unwrap();
    assert_eq!(checkpoint.epoch, 2);
    assert_eq!(checkpoint.conversations.len(), 1);
    assert_eq!(checkpoint.conversations[0].agent_id, agent.id);
    assert_eq!(
        captured
            .lifecycle
            .checkpoints
            .previous
            .as_ref()
            .unwrap()
            .epoch,
        1
    );
    harness.run(&current.id, retry_at).await;
    let parked_agent = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(parked_agent.status, AgentStatus::Paused);
    assert_eq!(parked_agent.revision, agent.revision + 1);
    assert_eq!(
        parked_agent.conversation.last_seq,
        agent.conversation.last_seq + u64::from(!parked)
    );
    for _ in 0..2 {
        harness.run(&current.id, retry_at).await;
    }
    assert_eq!(
        harness.stored(&current.id).await.status,
        SandboxStatus::Paused
    );
    let logs = harness.store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|log| matches!(
                log.kind,
                AgentLogKind::StatusChanged {
                    to: AgentStatus::Paused,
                    ..
                }
            ))
            .count(),
        1
    );
    let events = harness
        .store
        .sandbox_events(&current.id, 0, 200)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.kind,
                SandboxEventKind::AgentStatusChanged {
                    to: AgentStatus::Paused,
                    ..
                }
            ))
            .count(),
        1
    );
}

async fn unconfirmed_late_resume(parked: bool) {
    let (harness, current, agent, pause_at) = late_pause(parked).await;
    harness.backend.fail_next(
        WireOperation::CheckpointForTeardown,
        "invalid_checkpoint",
        "injected terminal hold error",
    );
    harness.backend.fail_next(
        WireOperation::ResumeCheckpointWriters,
        "invalid_checkpoint",
        "injected terminal resume error",
    );
    harness.run(&current.id, pause_at).await;
    let retry = harness.stored(&current.id).await;
    assert_eq!(retry.status, SandboxStatus::Pausing);
    assert!(retry.failure.is_none());
    assert_eq!(retry.lifecycle, current.lifecycle);
    assert_eq!(retry.work.attempts, current.work.attempts + 1);
    assert_eq!(
        harness.store.get_agent(&agent.id).await.unwrap().unwrap(),
        agent
    );
    let runtime = RuntimeHandle(current.runtime.as_ref().unwrap().runtime_id.clone());
    assert!(
        harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held
    );
    let retry_at = pause_at + Duration::from_secs(5);
    for _ in 0..if parked { 2 } else { 3 } {
        harness.run(&current.id, retry_at).await;
    }
    assert_eq!(
        harness.stored(&current.id).await.status,
        SandboxStatus::Paused
    );
    assert_eq!(
        harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Paused
    );
}

fn assert_pause_phase(sandbox: &Sandbox, expected: SandboxPausePhase) {
    assert!(
        matches!(sandbox.lifecycle.operation.as_ref(), Some(SandboxLifecycleOperation::Pause { phase, .. }) if *phase == expected)
    );
}
