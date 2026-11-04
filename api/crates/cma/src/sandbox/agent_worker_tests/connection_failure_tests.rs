//! Real recorder progress must not strand a worker's failed reconnect lease.
use super::super::agent_worker::WorkerFailure;
use super::*;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder as _};

mod capacity_cleanup_tests;

async fn claimed_after_metadata() -> (Harness, Agent, Agent) {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("reconnect-metadata-failure", vec![]).await;
    for _ in 0..4 {
        harness.run(&agent.id, harness.now).await;
    }
    let claimed = harness
        .store
        .claim_agent_work(
            &agent.id,
            "agent-worker",
            rfc3339_at(harness.now),
            rfc3339_at(harness.now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.step, AgentWorkStep::PublishConnection);
    harness
        .updates
        .record_update(
            &claimed.id,
            claimed.conversation.runtime_generation.unwrap(),
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::Said(layer3_adapter::SessionUpdate::AvailableCommandsUpdate(
                layer3_adapter::AvailableCommandsUpdate::new(vec![
                    layer3_adapter::AvailableCommand::new("review", "Review worktree"),
                ]),
            )),
            rfc3339_at(harness.now + Duration::from_secs(5)),
        )
        .await
        .unwrap();
    let updated = harness.stored(&claimed.id).await;
    assert!(updated.revision > claimed.revision);
    assert!(updated.conversation.last_seq > claimed.conversation.last_seq);
    (harness, claimed, updated)
}

fn failure() -> WorkerFailure {
    WorkerFailure::retryable("agent_runtime_unavailable", "reconnect did not complete")
}

#[tokio::test]
async fn reconnect_failure_after_recorded_metadata_retries_without_losing_progress() {
    let (harness, claimed, updated) = claimed_after_metadata().await;
    harness
        .worker
        .handle_failure(claimed.clone(), failure(), harness.now)
        .await
        .unwrap();
    let retry = harness.stored(&claimed.id).await;
    assert_eq!(retry.work.step, AgentWorkStep::PublishConnection);
    assert!(retry.work.lease.is_none());
    assert_eq!(retry.work.attempts, claimed.work.attempts + 1);
    assert!(retry.updated_at >= updated.updated_at);
    assert!(retry.work.ready_at > retry.updated_at);
    assert_eq!(retry.work.receipts, updated.work.receipts);
    let mut expected = updated.conversation.clone();
    expected.last_seq += 1;
    assert_eq!(retry.conversation, expected);
    let logs = harness
        .store
        .agent_log(&claimed.id, claimed.conversation.last_seq, 10)
        .await
        .unwrap();
    assert!(matches!(
        logs[0].kind,
        AgentLogKind::AvailableCommandsSet { .. }
    ));
    assert!(
        matches!(&logs[1].kind, AgentLogKind::WorkStepFailed { failure, .. }
        if failure.class == "agent_runtime_unavailable")
    );
}

#[tokio::test]
async fn reconnect_failure_cannot_release_expired_or_replaced_authority() {
    for replace in [false, true] {
        let (harness, claimed, updated) = claimed_after_metadata().await;
        let later = harness.now + Duration::from_secs(31);
        let expected = if replace {
            harness
                .store
                .claim_agent_work(
                    &claimed.id,
                    "replacement-worker",
                    rfc3339_at(later),
                    rfc3339_at(later + Duration::from_secs(30)),
                )
                .await
                .unwrap()
                .unwrap()
        } else {
            updated
        };
        assert!(matches!(
            harness
                .worker
                .handle_failure(claimed.clone(), failure(), later)
                .await,
            Err(SandboxStoreError::Conflict)
        ));
        assert_eq!(harness.stored(&claimed.id).await, expected);
    }
}
