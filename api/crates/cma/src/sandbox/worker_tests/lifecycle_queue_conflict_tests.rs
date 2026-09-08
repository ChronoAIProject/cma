use super::*;

use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
use layer2_core::sandbox::{
    Agent, AgentMessageAssembly, AgentUpdateRecorder, QueuedAgentMessage,
    SandboxLifecycleOperation, SandboxRevivePhase,
};
use layer3_adapter::{AvailableCommandsUpdate, ConversationUpdate, SessionUpdate};

use super::lifecycle_queue_tests::{begin_queue_phase, ready_agent};

async fn claimed_queue(harness: &Harness) -> (Sandbox, Agent, SystemTime) {
    let created = harness.create().await;
    let now = start_time(&created);
    harness.drive_active(&created.id, now).await;
    let agent = ready_agent(&created, now);
    harness.store.create_agent(agent.clone()).await.unwrap();
    let at = now + Duration::from_secs(1);
    begin_queue_phase(harness, &created.id, at).await;
    let claimed = harness
        .store
        .claim_sandbox_work(
            &created.id,
            "worker-a",
            rfc3339_at(at),
            rfc3339_at(at + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    (claimed, agent, at)
}

#[tokio::test]
async fn restored_metadata_conflict_releases_only_queue_completion_for_prompt_retry() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (claimed, stale_agent, now) = claimed_queue(&harness).await;
    DurableAgentUpdateRecorder::new(harness.store.clone())
        .record_update(
            &stale_agent.id,
            stale_agent.conversation.runtime_generation.unwrap(),
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::Said(SessionUpdate::AvailableCommandsUpdate(
                AvailableCommandsUpdate::new(Vec::new()),
            )),
            rfc3339_at(now),
        )
        .await
        .unwrap();
    let published = harness
        .store
        .get_agent(&stale_agent.id)
        .await
        .unwrap()
        .unwrap();
    assert!(published.revision > stale_agent.revision);
    assert!(published.conversation.last_seq > stale_agent.conversation.last_seq);
    let logs = harness
        .store
        .agent_log(&stale_agent.id, 0, 50)
        .await
        .unwrap();
    let requests = harness.backend.requests().await;
    let authority = claimed.lifecycle.operation.clone();

    harness
        .worker
        .complete_lifecycle_queue_for_test(claimed.clone(), vec![stale_agent.clone()], now)
        .await
        .unwrap();

    let retry = harness.stored(&claimed.id).await;
    assert_eq!(retry.lifecycle.operation, authority);
    assert_eq!(retry.runtime, claimed.runtime);
    assert_eq!(retry.work.step, claimed.work.step);
    assert_eq!(retry.work.next_fence, claimed.work.next_fence);
    assert_eq!(retry.work.attempts, claimed.work.attempts + 1);
    assert!(retry.work.lease.is_none());
    assert_eq!(
        retry.work.ready_at,
        rfc3339_at(now + Duration::from_secs(5))
    );
    assert_eq!(
        harness.store.get_agent(&stale_agent.id).await.unwrap(),
        Some(published)
    );
    assert_eq!(
        harness
            .store
            .agent_log(&stale_agent.id, 0, 50)
            .await
            .unwrap(),
        logs
    );
    assert_eq!(harness.backend.requests().await.len(), requests.len());
    assert!(!harness.worker.run_one(&claimed.id, now).await.unwrap());

    let retry_at = now + Duration::from_secs(5);
    harness.run(&claimed.id, retry_at).await;
    assert!(matches!(
        harness.stored(&claimed.id).await.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::PublishActive,
            ..
        })
    ));
    harness.run(&claimed.id, retry_at).await;
    assert_eq!(
        harness.stored(&claimed.id).await.status,
        SandboxStatus::Active
    );
    let final_agent = harness
        .store
        .get_agent(&stale_agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        final_agent.conversation.conversation_id,
        stale_agent.conversation.conversation_id
    );
    assert_eq!(
        harness
            .store
            .agent_log(&stale_agent.id, 0, 50)
            .await
            .unwrap(),
        logs
    );
}

#[tokio::test]
async fn queue_completion_conflict_preserves_concurrently_admitted_message() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (claimed, stale_agent, now) = claimed_queue(&harness).await;
    let mut queued = stale_agent.clone();
    queued.conversation.queued_messages.push(
        QueuedAgentMessage::new(1, "retain this admitted input".into(), rfc3339_at(now)).unwrap(),
    );
    queued.revision += 1;
    harness
        .store
        .save_agent(stale_agent.revision, queued.clone())
        .await
        .unwrap();
    harness
        .worker
        .complete_lifecycle_queue_for_test(claimed.clone(), vec![stale_agent], now)
        .await
        .unwrap();
    assert_eq!(
        harness.store.get_agent(&queued.id).await.unwrap(),
        Some(queued)
    );
    let retry = harness.stored(&claimed.id).await;
    assert_eq!(retry.lifecycle.operation, claimed.lifecycle.operation);
    assert!(retry.work.lease.is_none());
}

#[tokio::test]
async fn queue_completion_conflict_never_reschedules_a_changed_parent() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (claimed, agent, now) = claimed_queue(&harness).await;
    let mut changed = claimed.clone();
    changed.revision += 1;
    changed.updated_at = rfc3339_at(now + Duration::from_secs(1));
    harness
        .store
        .save_sandbox(claimed.revision, changed.clone())
        .await
        .unwrap();
    assert_eq!(
        harness
            .worker
            .complete_lifecycle_queue_for_test(claimed, vec![agent.clone()], now)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(harness.stored(&changed.id).await, changed);
    assert_eq!(
        harness.store.get_agent(&agent.id).await.unwrap(),
        Some(agent)
    );
}

#[tokio::test]
async fn queue_completion_conflict_never_releases_a_replacement_workers_lease() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (claimed, agent, now) = claimed_queue(&harness).await;
    let takeover_at = now + Duration::from_secs(61);
    let replacement = harness
        .store
        .claim_sandbox_work(
            &claimed.id,
            "worker-b",
            rfc3339_at(takeover_at),
            rfc3339_at(takeover_at + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        replacement.work.lease.as_ref().unwrap().fence > claimed.work.lease.as_ref().unwrap().fence
    );
    assert_eq!(
        harness
            .worker
            .complete_lifecycle_queue_for_test(claimed, vec![agent.clone()], now)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(harness.stored(&replacement.id).await, replacement);
    assert_eq!(
        harness.store.get_agent(&agent.id).await.unwrap(),
        Some(agent)
    );
}
