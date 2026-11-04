use super::*;

#[tokio::test]
async fn failed_open_settles_the_undispatched_first_message_and_retains_creation_replay() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness
        .admit_with_first_message("failed-open", vec![], Some("Wait for input"))
        .await;
    harness.run(&agent.id, harness.now).await;
    harness.run(&agent.id, harness.now).await;
    assert_eq!(
        harness.stored(&agent.id).await.work.step,
        AgentWorkStep::OpenConversation
    );
    harness
        .backend
        .fail_next(WireOperation::OpenAgent, "request_rejected", "open refused");
    harness.run(&agent.id, harness.now).await;
    let failed = harness.stored(&agent.id).await;
    assert_eq!(failed.status, AgentStatus::Failed);
    assert_eq!(failed.work.step, AgentWorkStep::None);
    let first = failed.conversation.first_message.as_ref().unwrap();
    assert_eq!(first.state, FirstMessageDelivery::Failed);
    assert_eq!(first.failure.as_ref().unwrap().code, "agent_startup_failed");
    assert!(!first.failure.as_ref().unwrap().retryable);
    assert_eq!(first.attempt, 0);
    assert_eq!(first.turn_sequence, None);
    assert_eq!(
        first.payload_sha256,
        agent
            .conversation
            .first_message
            .as_ref()
            .unwrap()
            .payload_sha256
    );
    let replay = harness
        .store
        .admit_agent_reusing_profile(AgentAdmission {
            binding: None,
            created_by: None,
            owner: agent.owner.clone(),
            workspace_id: agent.workspace_id.clone(),
            agent_id: AgentId::generate(),
            worktree_id: WorktreeId::generate(),
            profile: agent.profile.clone(),
            capabilities: agent.capabilities.clone(),
            title: None,
            first_message: agent.conversation.first_message.clone(),
            origin: agent.origin.clone(),
            creation_key: agent.creation_key.clone(),
            event_at: agent.created_at.clone(),
        })
        .await
        .unwrap();
    let AgentAdmissionResult::Existing(replay) = replay else {
        panic!("must replay creation")
    };
    assert_eq!(replay.agent, failed);
    assert!(!harness
        .worker
        .run_one(&agent.id, harness.now + Duration::from_secs(10))
        .await
        .unwrap());
    assert_eq!(harness.stored(&agent.id).await, failed);
    let logs = harness.store.agent_log(&agent.id, 0, 20).await.unwrap();
    assert_eq!(logs.len(), 4);
    assert!(matches!(
        logs.last().unwrap().kind,
        AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::OpenConversation,
            ..
        }
    ));
}

#[tokio::test]
async fn unavailable_open_keeps_the_first_message_queued_until_successful_retry() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness
        .admit_with_first_message("retry-open", vec![], Some("Wait for input"))
        .await;
    harness.run(&agent.id, harness.now).await;
    harness.run(&agent.id, harness.now).await;
    harness.backend.lose_next_response(WireOperation::OpenAgent);
    harness.run(&agent.id, harness.now).await;
    let retrying = harness.stored(&agent.id).await;
    assert_eq!(retrying.status, AgentStatus::Starting);
    assert_eq!(
        retrying.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::Queued
    );
    let retry_at = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&retrying.work.ready_at).unwrap());
    tokio::time::sleep(
        retry_at
            .duration_since(SystemTime::now())
            .unwrap_or_default(),
    )
    .await;
    for attempt in 0..4 {
        let result = harness.worker.run_one(&agent.id, SystemTime::now()).await;
        assert!(result.is_ok(), "retry {attempt}: {result:?}");
    }
    let ready = harness.stored(&agent.id).await;
    assert_eq!(
        ready.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::WriteConfirmed
    );
    assert_eq!(
        ready.conversation.first_message.as_ref().unwrap().attempt,
        1
    );
}
