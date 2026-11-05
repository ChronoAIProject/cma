use super::*;
use layer2_core::sandbox::store::AGENT_LOG_CAPACITY_FAILURE;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn full_history_working_reconnect_preserves_dispatched_input_and_active_turn() {
    use layer2_core::sandbox::store::{
        AgentConnectionRecoveryCandidate, AgentConnectionRecoveryOutcome,
    };
    let (client, database, harness) = fixture().await;
    let agent = harness
        .admit_with_first_message("capacity-working", vec![], Some("already dispatched input"))
        .await;
    for _ in 0..6 {
        harness.run(&agent.id, SystemTime::now()).await;
    }
    let full = fill_history(&client, &database, &harness, &agent.id).await;
    assert_eq!(full.work.step, AgentWorkStep::None);
    assert!(full.conversation.active_turn.is_some());
    assert_eq!(full.conversation.first_message.as_ref().unwrap().attempt, 1);
    assert_eq!(
        harness
            .store
            .schedule_agent_connection_recovery(
                AgentConnectionRecoveryCandidate::from_agent(&full)
                    .unwrap()
                    .restart_request(rfc3339_at(SystemTime::now())),
            )
            .await
            .unwrap(),
        AgentConnectionRecoveryOutcome::Scheduled,
    );
    harness.run(&agent.id, SystemTime::now()).await;
    let failed = harness.stored(&agent.id).await;
    assert_eq!(failed.status, AgentStatus::Failed);
    assert_eq!(
        failed.failure.as_ref().unwrap().class,
        AGENT_LOG_CAPACITY_FAILURE
    );
    assert_eq!(
        failed.conversation.first_message,
        full.conversation.first_message
    );
    assert_eq!(
        failed.conversation.active_turn,
        full.conversation.active_turn
    );
    assert_eq!(
        failed.conversation.queued_messages,
        full.conversation.queued_messages
    );
    assert_eq!(failed.work.receipts, full.work.receipts);
    assert_eq!(failed.work.cleanup_debt, full.work.cleanup_debt);
    assert!(failed.work.lease.is_none());
    assert_eq!(failed.work.step, AgentWorkStep::None);
    assert_eq!(failed.conversation.last_seq, 10_001);
    assert!(!harness
        .worker
        .run_one(&agent.id, SystemTime::now() + Duration::from_secs(1_100))
        .await
        .unwrap());
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(reopened.get_agent(&agent.id).await.unwrap(), Some(failed));
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn full_history_startup_settles_without_discarding_effect_intent_or_sending_input() {
    for (steps, expected_step) in [
        (2, AgentWorkStep::OpenConversation),
        (3, AgentWorkStep::PublishReady),
        (5, AgentWorkStep::DispatchFirstMessage),
    ] {
        let (client, database, harness) = fixture().await;
        let agent = harness
            .admit_with_first_message("capacity-startup", vec![], Some("retained first input"))
            .await;
        for _ in 0..steps {
            harness.run(&agent.id, SystemTime::now()).await;
        }
        let full = fill_history(&client, &database, &harness, &agent.id).await;
        assert_eq!(full.work.step, expected_step);
        harness.run(&agent.id, SystemTime::now()).await;
        let failed = harness.stored(&agent.id).await;
        assert_eq!(failed.status, AgentStatus::Failed);
        assert_eq!(
            failed.failure.as_ref().unwrap().class,
            AGENT_LOG_CAPACITY_FAILURE
        );
        assert_eq!(failed.work.step, AgentWorkStep::None);
        assert!(failed.work.lease.is_none());
        assert_eq!(failed.work.receipts, full.work.receipts);
        for debt in &full.work.cleanup_debt {
            assert!(failed.work.cleanup_debt.contains(debt));
        }
        if expected_step == AgentWorkStep::OpenConversation {
            // Opening the native process committed its intent before the log
            // rejected completion. Retain that debt for explicit cleanup.
            assert!(failed
                .work
                .cleanup_debt
                .iter()
                .any(|debt| debt.step == expected_step));
        }
        let first = failed.conversation.first_message.as_ref().unwrap();
        assert_eq!(first.state, FirstMessageDelivery::Failed);
        assert_eq!(first.attempt, 0);
        assert!(failed.stopped_at.is_none());
        assert!(failed.deleted_at.is_none());
        assert_eq!(failed.conversation.last_seq, 10_001);
        client.database(&database).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn full_history_reconnect_settles_before_lease_expiry_and_retains_native_home() {
    let (client, database, harness) = fixture().await;
    let agent = harness
        .admit_with_first_message(
            "capacity-reconnect",
            vec![],
            Some("Do not resend this input"),
        )
        .await;
    for _ in 0..4 {
        harness.run(&agent.id, SystemTime::now()).await;
    }
    let full = fill_history(&client, &database, &harness, &agent.id).await;
    assert_eq!(full.work.step, AgentWorkStep::PublishConnection);
    let before = harness.backend.requests().await.len();
    let started = std::time::Instant::now();
    harness.run(&agent.id, SystemTime::now()).await;
    assert!(started.elapsed() < Duration::from_secs(30));
    let failed = harness.stored(&agent.id).await;
    assert_eq!(failed.status, AgentStatus::Failed);
    assert_eq!(
        failed.failure.as_ref().unwrap().class,
        AGENT_LOG_CAPACITY_FAILURE
    );
    assert_eq!(failed.work.step, AgentWorkStep::None);
    assert!(failed.work.lease.is_none());
    assert_eq!(failed.conversation.last_seq, 10_001);
    assert_eq!(
        failed.conversation.conversation_id,
        full.conversation.conversation_id
    );
    assert_eq!(
        failed.conversation.runtime_generation,
        full.conversation.runtime_generation
    );
    assert_eq!(failed.work.receipts, full.work.receipts);
    assert_eq!(failed.work.cleanup_debt, full.work.cleanup_debt);
    let first = failed.conversation.first_message.as_ref().unwrap();
    assert_eq!(first.state, FirstMessageDelivery::Failed);
    assert_eq!(first.attempt, 0);
    assert_eq!(first.failure.as_ref().unwrap().code, "agent_startup_failed");
    assert!(failed.stopped_at.is_none());
    assert!(failed.deleted_at.is_none());
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert!(snapshot
        .homes
        .contains_key(&AgentSlot::new(agent.id.to_string())));
    assert!(!harness.backend.requests().await[before..]
        .iter()
        .any(|(_, request)| matches!(
            request.operation(),
            WireOperation::CloseAgent
                | WireOperation::RemoveAgentHome
                | WireOperation::RemoveWorktree
        )));
    assert!(!harness
        .worker
        .run_one(&agent.id, SystemTime::now() + Duration::from_secs(19 * 60))
        .await
        .unwrap());
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(reopened.get_agent(&agent.id).await.unwrap(), Some(failed));
    let tail = reopened.agent_log(&agent.id, 9_999, 20).await.unwrap();
    assert_eq!(tail.len(), 2);
    assert!(matches!(
        tail[0].kind,
        AgentLogKind::ToolOutputReported { .. }
    ));
    assert!(
        matches!(&tail[1].kind, AgentLogKind::WorkStepFailed { failure, .. } if failure.class == AGENT_LOG_CAPACITY_FAILURE)
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn full_history_close_failure_releases_lease_and_retries_original_effect() {
    let (client, database, harness) = fixture().await;
    let agent = harness.admit("capacity-close-retry", vec![]).await;
    for _ in 0..5 {
        harness.run(&agent.id, SystemTime::now()).await;
    }
    let full = fill_history(&client, &database, &harness, &agent.id).await;
    harness
        .store
        .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
            target: AgentControlTarget::from(&full),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(SystemTime::now()),
        })
        .await
        .unwrap();
    harness.run(&agent.id, SystemTime::now()).await;
    // Native close succeeds, but its acknowledgement is lost. No completion
    // receipt may be invented while recording the bounded capacity retry.
    harness
        .backend
        .lose_next_response(WireOperation::CloseAgent);
    harness.run(&agent.id, SystemTime::now()).await;
    let retry = harness.stored(&agent.id).await;
    assert_eq!(retry.work.step, AgentWorkStep::CloseProcess);
    assert_eq!(retry.work.attempts, 1);
    assert!(retry.work.lease.is_none());
    assert!(!retry
        .work
        .receipts
        .iter()
        .any(|r| r.step == AgentWorkStep::CloseProcess));
    assert!(retry.stopped_at.is_none());
    let due = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&retry.work.ready_at).unwrap());
    assert!(due.duration_since(SystemTime::now()).unwrap_or_default() < Duration::from_secs(60));
    harness.run(&agent.id, due).await;
    harness.run(&agent.id, due + Duration::from_secs(1)).await;
    let stopped = harness.stored(&agent.id).await;
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert!(stopped
        .work
        .receipts
        .iter()
        .any(|r| r.step == AgentWorkStep::CloseProcess));
    assert_eq!(stopped.conversation.last_seq, 10_004);
    client.database(&database).drop().await.unwrap();
}
