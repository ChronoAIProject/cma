use super::*;

#[tokio::test(start_paused = true)]
async fn desktop_retry_stays_due_after_publication_reads_cross_a_second_boundary() {
    for (read_ms, retry_ms) in [
        (0, 1_000),
        (200, 1_000),
        (2_100, 1_000),
        (30_000, 1_000),
        (200, 0),
    ] {
        let read_delay = Duration::from_millis(read_ms);
        let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let seconds = harness
            .now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        harness.now =
            SystemTime::UNIX_EPOCH + Duration::from_secs(seconds + 1) + Duration::from_millis(900);
        let admitted = harness
            .admit_with_first_message(
                "desktop-retry-boundary",
                vec![],
                Some("Keep the original task"),
            )
            .await;
        for _ in 0..3 {
            harness.run(&admitted.id, harness.now).await;
        }
        let claimed = harness
            .store
            .claim_agent_work(
                &admitted.id,
                "agent-worker",
                rfc3339_at(harness.now),
                rfc3339_at(harness.now + Duration::from_secs(30)),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.work.step, AgentWorkStep::PublishReady);
        let store = lifecycle_failure_tests::fixture::Store::new(harness.store.clone());
        store.delay_read("get_sandbox", 1, read_delay);
        let mut config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
        config.retry_delay = Duration::from_millis(retry_ms);
        let worker = AgentWorker::new(
            AgentWorkerDependencies {
                store: store.clone(),
                adapter: harness.adapter.clone(),
                skills: Arc::new(EmptyAgentSkillResolver),
                registry: harness.registry.clone(),
                updates: harness.updates.clone(),
                operations: AgentOperationService::new(
                    harness.store.clone(),
                    harness.registry.clone(),
                ),
            },
            config,
        );
        let requests = harness.backend.requests().await;
        let result = worker
            .handle_failure(
                claimed.clone(),
                super::super::agent_worker::WorkerFailure::retryable(
                    "desktop_preparation_unavailable",
                    "synthetic lost browser receipt",
                ),
                harness.now,
            )
            .await;
        assert_eq!(harness.backend.requests().await, requests);
        if read_delay >= Duration::from_secs(30) {
            assert_eq!(
                result,
                Err(layer2_core::sandbox::store::SandboxStoreError::Conflict)
            );
            assert_eq!(harness.stored(&claimed.id).await, claimed);
            assert!(!store
                .calls
                .lock()
                .unwrap()
                .contains_key("commit_agent_work"));
            continue;
        }
        result.unwrap();
        let retry = harness.stored(&claimed.id).await;
        assert_eq!(retry.updated_at, rfc3339_at(harness.now + read_delay));
        assert_eq!(
            retry.work.ready_at,
            rfc3339_at(harness.now + read_delay + Duration::from_secs(1))
        );
        assert_eq!(retry.work.attempts, 1);
        assert!(retry.work.lease.is_none());
        assert_eq!(retry.work.step, claimed.work.step);
        assert_eq!(retry.work.receipts, claimed.work.receipts);
        assert_eq!(retry.work.cleanup_debt, claimed.work.cleanup_debt);
        assert_eq!(
            retry.conversation.first_message,
            claimed.conversation.first_message
        );
        assert_eq!(
            retry.conversation.conversation_id,
            claimed.conversation.conversation_id
        );
        assert_eq!(
            store.calls.lock().unwrap().get("commit_agent_work"),
            Some(&1)
        );
    }
}

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
