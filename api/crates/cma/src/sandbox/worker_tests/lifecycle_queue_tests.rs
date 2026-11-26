use super::*;

use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, PermissionLevel, ThinkingEffort,
};
use layer2_core::sandbox::store::{AgentStore, SandboxWorkerStore};
use layer2_core::sandbox::{
    Agent, AgentMessageAssembly, AgentMessageOutcome, AgentOperationDelivery, AgentOrigin,
    AgentStatus, AgentUpdateRecorder, AgentWorkState, AgentWorkStep, ConversationState,
    QueuedMessageState, SandboxCheckpointRef, SandboxLifecycleOperation, SandboxRevivePhase,
    QUEUED_MESSAGE_DISPATCH_GRACE_SECONDS,
};
use layer2_core::{AgentId, AgentProfileId, RuntimeCapability};
use layer3_adapter::{
    CheckpointReceipt, ConversationInput, ConversationInputSender, ConversationUpdate,
    PromptResponse, TurnStopReason, WriterState,
};

#[tokio::test]
async fn reopened_agent_settlement_rechecks_promptly_without_duplicate_runtime_work() {
    for delay in [1, 60] {
        let harness = Harness::new_with_retry_delay(
            Vec::new(),
            0,
            Duration::from_secs(60),
            Duration::from_secs(delay),
        )
        .await;
        let created = harness.create().await;
        let now = start_time(&created);
        harness.drive_active(&created.id, now).await;
        let mut agent = ready_agent(&created, now);
        agent.work.step = AgentWorkStep::PublishConnection;
        harness.store.create_agent(agent.clone()).await.unwrap();
        let queue_at = now + Duration::from_secs(1);
        begin_queue_phase(&harness, &created.id, queue_at).await;
        let requests = harness.backend.requests().await;

        harness.run(&created.id, queue_at).await;
        let retry = harness.stored(&created.id).await;
        let retry_at = queue_at + Duration::from_secs(delay.min(5));
        assert_eq!(retry.work.ready_at, rfc3339_at(retry_at));
        assert!(retry.work.lease.is_none());
        assert_eq!(
            harness.store.get_agent(&agent.id).await.unwrap(),
            Some(agent.clone())
        );
        assert_eq!(harness.backend.requests().await, requests);
        assert!(!harness.worker.run_one(&created.id, queue_at).await.unwrap());

        let revision = agent.revision;
        agent.work.step = AgentWorkStep::None;
        agent.revision += 1;
        harness.store.save_agent(revision, agent).await.unwrap();
        harness.run(&created.id, retry_at).await;
        harness.run(&created.id, retry_at).await;
        assert_eq!(
            harness.stored(&created.id).await.status,
            SandboxStatus::Active
        );
        assert_eq!(harness.backend.requests().await, requests);
    }
}

#[tokio::test]
async fn revived_worker_delivers_each_queued_message_once_then_publishes_active() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    harness.drive_active(&created.id, now).await;
    let agent = ready_agent(&created, now);
    harness.store.create_agent(agent.clone()).await.unwrap();
    let queue_at = now + Duration::from_secs(1);
    begin_queue_phase(&harness, &created.id, queue_at).await;

    let inputs = Arc::new(Mutex::new(Vec::new()));
    let captured = inputs.clone();
    let input = ConversationInputSender::spawn(4, move |input| {
        captured.lock().unwrap().push(input);
        async { Ok(()) }
    });
    let (updates, receiver) = tokio::sync::mpsc::channel(4);
    let attachment = harness
        .registry
        .attach(
            agent.id.clone(),
            agent.conversation.conversation_id.clone().unwrap(),
            agent.conversation.runtime_generation.unwrap(),
            agent.work.next_fence,
            layer2_core::RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input,
            receiver,
            DurableAgentUpdateRecorder::new(harness.store.clone()),
        )
        .unwrap();
    harness.registry.publish_and_activate(&attachment).unwrap();
    let operations = AgentOperationService::new(harness.store.clone(), harness.registry.clone());
    assert!(matches!(
        operations
            .send_message(
                &caller("owner", [Capability::UseOwnSandboxes]),
                &agent.id,
                "queued during revive".into(),
            )
            .await
            .unwrap(),
        AgentMessageOutcome::Queued { .. }
    ));

    harness.run(&created.id, queue_at).await;
    let dispatched = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(dispatched.status, AgentStatus::Working);
    assert_eq!(
        dispatched.conversation.queued_messages[0].state,
        QueuedMessageState::Delivered
    );
    assert!(matches!(
        inputs.lock().unwrap().as_slice(),
        [ConversationInput::RecoverablePrompt { .. }]
    ));

    DurableAgentUpdateRecorder::new(harness.store.clone())
        .record_update(
            &agent.id,
            dispatched.conversation.runtime_generation.unwrap(),
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            rfc3339_at(queue_at + Duration::from_secs(1)),
        )
        .await
        .unwrap();
    drop(updates);

    harness
        .run(&created.id, queue_at + Duration::from_secs(2))
        .await;
    let pruned = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert!(pruned.conversation.queued_messages.is_empty());
    assert!(matches!(
        harness.stored(&created.id).await.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::PublishActive,
            ..
        })
    ));

    harness
        .run(&created.id, queue_at + Duration::from_secs(3))
        .await;
    assert_eq!(
        harness.stored(&created.id).await.status,
        SandboxStatus::Active
    );
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn restarted_worker_retires_an_abandoned_queue_fence_without_resending() {
    let harness = Harness::new_with_retry_delay(
        Vec::new(),
        0,
        Duration::from_secs(60),
        Duration::from_secs(60),
    )
    .await;
    let created = harness.create().await;
    let now = start_time(&created);
    harness.drive_active(&created.id, now).await;
    let agent = ready_agent(&created, now);
    harness.store.create_agent(agent.clone()).await.unwrap();
    let queue_at = now + Duration::from_secs(1);
    begin_queue_phase(&harness, &created.id, queue_at).await;

    let inputs = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let input = ConversationInputSender::spawn(4, {
        let inputs = inputs.clone();
        let entered = entered.clone();
        let release = release.clone();
        move |input| {
            let inputs = inputs.clone();
            let entered = entered.clone();
            let release = release.clone();
            async move {
                inputs.lock().unwrap().push(input);
                entered.notify_one();
                release.notified().await;
                Ok(())
            }
        }
    });
    let (_updates, receiver) = tokio::sync::mpsc::channel(4);
    let attachment = harness
        .registry
        .attach(
            agent.id.clone(),
            agent.conversation.conversation_id.clone().unwrap(),
            agent.conversation.runtime_generation.unwrap(),
            agent.work.next_fence,
            layer2_core::RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input,
            receiver,
            DurableAgentUpdateRecorder::new(harness.store.clone()),
        )
        .unwrap();
    harness.registry.publish_and_activate(&attachment).unwrap();
    let operations = AgentOperationService::new(harness.store.clone(), harness.registry.clone());
    let sequence = match operations
        .send_message(
            &caller("owner", [Capability::UseOwnSandboxes]),
            &agent.id,
            "fenced before restart".into(),
        )
        .await
        .unwrap()
    {
        AgentMessageOutcome::Queued { sequence, .. } => sequence,
        AgentMessageOutcome::Dispatched(_) => panic!("recovery input bypassed the queue"),
    };
    let claimed = harness
        .store
        .claim_sandbox_work(
            &created.id,
            "departing-worker",
            rfc3339_at(queue_at),
            rfc3339_at(queue_at + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    let dispatch = tokio::spawn({
        let operations = operations.clone();
        let agent_id = agent.id.clone();
        async move {
            operations
                .dispatch_lifecycle_queued_message(
                    &claimed,
                    &agent_id,
                    sequence,
                    rfc3339_at(queue_at),
                )
                .await
        }
    });
    entered.notified().await;
    dispatch.abort();
    assert!(dispatch.await.unwrap_err().is_cancelled());
    release.notify_one();
    tokio::task::yield_now().await;

    let before_grace = queue_at + Duration::from_secs(QUEUED_MESSAGE_DISPATCH_GRACE_SECONDS - 100);
    harness.run(&created.id, before_grace).await;
    assert_eq!(
        harness.stored(&created.id).await.work.ready_at,
        rfc3339_at(before_grace + Duration::from_secs(60)),
    );
    let still_fenced = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(
        still_fenced.conversation.queued_messages[0].state,
        QueuedMessageState::DispatchFenced
    );

    let recovery_at = queue_at + Duration::from_secs(QUEUED_MESSAGE_DISPATCH_GRACE_SECONDS);
    harness.run(&created.id, recovery_at).await;
    let recovered = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(recovered.status, AgentStatus::Working);
    assert_eq!(
        recovered.conversation.operation.as_ref().unwrap().state,
        AgentOperationDelivery::RecoveryRequired
    );
    assert_eq!(
        recovered.conversation.queued_messages[0].state,
        QueuedMessageState::RecoveryRequired
    );
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

pub(super) fn ready_agent(sandbox: &Sandbox, now: SystemTime) -> Agent {
    let stamp = rfc3339_at(now);
    Agent {
        id: AgentId::generate(),
        worktree_id: layer2_core::WorktreeId::generate(),
        workspace_id: layer2_core::WorkspaceId::generate(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        creation_key: "lifecycle-queue-agent".into(),
        creation_fingerprint: "f".repeat(64),
        profile: queue_agent_profile(),
        capabilities: layer2_core::RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        status: AgentStatus::Ready,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: Some("conversation-lifecycle-queue".into()),
            runtime_generation: Some(3),
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: Vec::new(),
        },
        work: AgentWorkState::new(AgentWorkStep::None, stamp.clone()),
        failure: None,
        title: None,
        created_at: stamp.clone(),
        updated_at: stamp.clone(),
        ready_at: Some(stamp),
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}

fn queue_agent_profile() -> AgentProfileSnapshot {
    AgentProfileSnapshot {
        id: AgentProfileId::generate(),
        revision: 1,
        digest: "d".repeat(64),
        content: AgentProfileContent {
            isolated_agent: false,
            model: Some("gpt-test".into()),
            thinking_effort: ThinkingEffort::High,
            permission_level: PermissionLevel::Autonomous,
            skills: Vec::new(),
            system_prompt: "Be exact.".into(),
        },
    }
}

pub(super) async fn begin_queue_phase(harness: &Harness, id: &SandboxId, now: SystemTime) {
    let current = harness.stored(id).await;
    let runtime = current.runtime.clone().unwrap();
    let stamp = rfc3339_at(now);
    let checkpoint = SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch: 1,
            object_reference: "checkpoint/lifecycle-queue".into(),
            sha256: "e".repeat(64),
            writers: WriterState::Resumed,
        },
        &[],
        stamp.clone(),
    )
    .unwrap();
    let mut next = current.clone();
    next.lifecycle.checkpoints.publish(checkpoint).unwrap();
    next.status = SandboxStatus::Paused;
    next.runtime = None;
    next.work.schedule(SandboxWorkStep::None, stamp.clone());
    next.begin_revive(SandboxReviveCause::AgentMessage, stamp.clone())
        .unwrap();
    let Some(SandboxLifecycleOperation::Revive { phase, .. }) = next.lifecycle.operation.as_mut()
    else {
        unreachable!()
    };
    *phase = SandboxRevivePhase::QueuedMessageDelivery;
    next.runtime = Some(runtime);
    next.validate().unwrap();
    harness
        .store
        .save_sandbox(current.revision, next)
        .await
        .unwrap();
}
