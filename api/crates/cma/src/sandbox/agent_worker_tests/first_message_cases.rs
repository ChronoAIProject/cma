use super::*;

#[tokio::test(start_paused = true)]
async fn worker_dispatches_a_configured_first_message_and_retires_lifecycle_work() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness
        .admit_with_first_message("first-message", Vec::new(), Some("Inspect the workspace"))
        .await;
    for _ in 0..6 {
        harness.run(&agent.id, harness.now).await;
    }

    let dispatched = harness.stored(&agent.id).await;
    let first = dispatched.conversation.first_message.as_ref().unwrap();
    assert_eq!(dispatched.status, AgentStatus::Working);
    assert_eq!(dispatched.work.step, AgentWorkStep::None);
    assert_eq!(first.state, FirstMessageDelivery::WriteConfirmed);
    assert_eq!(first.attempt, 1);
    let logs = harness.store.agent_log(&agent.id, 0, 40).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn restart_reattaches_without_overwriting_first_message_work() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness
        .admit_with_first_message(
            "restart-first-message",
            Vec::new(),
            Some("Inspect after restart"),
        )
        .await;
    for _ in 0..5 {
        harness.run(&agent.id, harness.now).await;
    }
    let dispatching = harness.stored(&agent.id).await;
    assert_eq!(dispatching.work.step, AgentWorkStep::DispatchFirstMessage);
    assert_eq!(
        dispatching
            .conversation
            .first_message
            .as_ref()
            .unwrap()
            .state,
        FirstMessageDelivery::Queued
    );
    harness.registry.fence(
        &dispatching.id,
        dispatching.conversation.runtime_generation,
        dispatching.work.next_fence - 1,
    );

    assert_eq!(
        harness
            .worker
            .reconcile_connections(harness.now)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        harness.stored(&agent.id).await.work.step,
        AgentWorkStep::DispatchFirstMessage
    );
    harness.run(&agent.id, harness.now).await;

    let recovered = harness.stored(&agent.id).await;
    assert_eq!(recovered.status, AgentStatus::Working);
    assert_eq!(recovered.work.step, AgentWorkStep::None);
    assert_eq!(
        recovered.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::WriteConfirmed
    );
    assert_eq!(
        harness
            .store
            .agent_log(&agent.id, 0, 40)
            .await
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn worker_completes_an_unconfigured_dispatch_step_without_runtime_input() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness.admit("unconfigured-dispatch", Vec::new()).await;
    let mut ready = harness.drive_ready(&agent.id).await;
    let expected = ready.revision;
    ready
        .work
        .schedule(AgentWorkStep::DispatchFirstMessage, rfc3339_at(harness.now));
    ready.revision += 1;
    harness.store.save_agent(expected, ready).await.unwrap();

    harness.run(&agent.id, harness.now).await;
    let completed = harness.stored(&agent.id).await;
    assert_eq!(completed.status, AgentStatus::Ready);
    assert_eq!(completed.work.step, AgentWorkStep::None);
    assert!(completed.conversation.first_message.is_none());
}

#[tokio::test(start_paused = true)]
async fn worker_retries_only_a_proven_first_message_pre_write_failure() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness
        .admit_with_first_message("retry-first", Vec::new(), Some("Run checks"))
        .await;
    for _ in 0..5 {
        harness.run(&agent.id, harness.now).await;
    }
    let dispatching = harness.stored(&agent.id).await;
    assert_eq!(dispatching.work.step, AgentWorkStep::DispatchFirstMessage);

    let closed = ConversationInputSender::spawn(1, |_| async {
        Err(AdapterError::Unavailable("retired writer".into()))
    });
    assert!(closed.send(ConversationInput::Cancel).await.is_err());
    tokio::task::yield_now().await;
    let _updates = harness.replace_input(&dispatching, dispatching.work.next_fence, closed);
    harness.run(&agent.id, harness.now).await;

    let retrying = harness.stored(&agent.id).await;
    assert_eq!(retrying.status, AgentStatus::Ready);
    assert_eq!(retrying.work.step, AgentWorkStep::DispatchFirstMessage);
    assert_eq!(retrying.work.attempts, 1);
    assert_eq!(
        retrying.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::Queued
    );

    let working = ConversationInputSender::spawn(1, |_| async { Ok(()) });
    let _updates = harness.replace_input(&retrying, retrying.work.next_fence, working);
    let retry_at = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&retrying.work.ready_at).unwrap());
    harness.run(&agent.id, retry_at).await;
    let accepted = harness.stored(&agent.id).await;
    assert_eq!(accepted.work.step, AgentWorkStep::None);
    assert_eq!(
        accepted.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::WriteConfirmed
    );
}

#[tokio::test(start_paused = true)]
async fn worker_retires_ambiguous_first_message_work_into_explicit_recovery() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness
        .admit_with_first_message("ambiguous-first", Vec::new(), Some("Run migration"))
        .await;
    for _ in 0..5 {
        harness.run(&agent.id, harness.now).await;
    }
    let dispatching = harness.stored(&agent.id).await;
    let ambiguous = ConversationInputSender::spawn(1, |_| async {
        Err(AdapterError::Unavailable(
            "receipt lost after admission".into(),
        ))
    });
    let _updates = harness.replace_input(&dispatching, dispatching.work.next_fence, ambiguous);

    harness.run(&agent.id, harness.now).await;
    let recovery = harness.stored(&agent.id).await;
    assert_eq!(recovery.status, AgentStatus::Working);
    assert_eq!(recovery.work.step, AgentWorkStep::None);
    assert_eq!(
        recovery.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::RecoveryRequired
    );
}

#[tokio::test(start_paused = true)]
async fn worker_does_not_resend_a_first_message_fenced_by_the_same_lease() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness
        .admit_with_first_message("in-progress-first", Vec::new(), Some("Keep running"))
        .await;
    for _ in 0..5 {
        harness.run(&agent.id, harness.now).await;
    }
    let stamp = rfc3339_at(harness.now);
    let claimed = harness
        .store
        .claim_agent_work(
            &agent.id,
            "agent-worker",
            stamp.clone(),
            rfc3339_at(harness.now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let expected_last_seq = claimed.conversation.last_seq;
    let mut fenced = claimed.clone();
    let fence_sequence = expected_last_seq + 1;
    let turn_sequence = fence_sequence + 1;
    fenced
        .conversation
        .first_message
        .as_mut()
        .unwrap()
        .fence_dispatch_for(turn_sequence, fence_sequence, fence, stamp.clone())
        .unwrap();
    fenced.conversation.active_turn = Some(turn_sequence);
    fenced
        .transition(AgentStatus::Working, stamp.clone(), None)
        .unwrap();
    let logs = vec![
        AgentLogKind::FirstMessageChanged {
            state: FirstMessageDelivery::DispatchFenced,
            recovery_generation: 1,
        },
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence,
            origin_key: Some("in-progress-first".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "Keep running".into(),
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Ready,
            to: AgentStatus::Working,
        },
    ];
    fenced.conversation.last_seq += u64::try_from(logs.len()).unwrap();
    let parent = layer2_core::sandbox::store::SandboxStore::get_sandbox(
        harness.store.as_ref(),
        &claimed.sandbox_id,
    )
    .await
    .unwrap()
    .unwrap();
    harness
        .store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: Some(layer2_core::sandbox::store::SandboxMutationGuard::capture(
                &parent,
            )),
            expected_agent_revision: claimed.revision,
            expected_last_seq,
            runtime_generation: claimed.conversation.runtime_generation.unwrap(),
            agent: fenced.clone(),
            appended_at: stamp,
            agent_logs: logs,
        })
        .await
        .unwrap();

    harness
        .worker
        .dispatch_first_message(claimed, harness.now)
        .await
        .unwrap();
    assert_eq!(harness.stored(&agent.id).await, fenced);
}
