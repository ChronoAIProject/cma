use super::*;
use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::agent_ownership::{
    execution_owner, Member, OwnershipCoordinator, OwnershipError, OwnershipStore, PROTOCOL_VERSION,
};
use std::time::{Duration, UNIX_EPOCH};

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn stale_manager_cannot_publish_trigger_failure_after_handoff() {
    Box::pin(run_handoff(true)).await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn successor_finishes_trigger_once_after_handoff() {
    Box::pin(run_handoff(false)).await;
}

async fn run_handoff(publish_stale: bool) {
    let accepted = Accepted::new().await;
    let seed = &accepted.f.seed;
    let peer = mongodb::Client::with_uri_str(
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("use disposable Mongo"),
    )
    .await
    .unwrap();
    let one = MongoSandboxStore::open(&seed.client, &seed.database)
        .await
        .unwrap()
        .enable_agent_ownership("manager-a".into())
        .await
        .unwrap();
    let two = Arc::new(
        MongoSandboxStore::open(&peer, &seed.database)
            .await
            .unwrap()
            .enable_agent_ownership("manager-b".into())
            .await
            .unwrap(),
    );
    let owners_a = one.agent_ownership.as_ref().unwrap();
    let owners_b = two.agent_ownership.as_ref().unwrap();
    let manager_a = register(owners_a.clone(), "manager-a").await;
    let id = &accepted.agent.id;
    let first = manager_a.acquire(id).await.unwrap().unwrap();
    let manager_b = register(owners_b.clone(), "manager-b").await;
    assert!(manager_b.acquire(id).await.unwrap().is_none());

    let decision_at = accepted.command.receipt.admitted_at.clone();
    let decision_secs = layer2_core::seconds_from_rfc3339(&decision_at).unwrap();
    let claim_expiry =
        layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(decision_secs as u64 + 1));
    let successor_expiry =
        layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(decision_secs as u64 + 61));
    let before = one.get_agent(id).await.unwrap().unwrap();
    assert_eq!(
        before.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::Queued
    );
    let claimed = one
        .claim_next_command(CommandClaimRequest {
            agent_id: id.clone(),
            holder: "manager-a".into(),
            now: decision_at.clone(),
            expires_at: claim_expiry.clone(),
        })
        .await
        .unwrap()
        .unwrap();
    let original_claim = &claimed.work.lease.as_ref().unwrap().claim;
    let command_id = &original_claim.command_id;

    let (successor, stale_result) = first
        .run(async {
            owners_a.check(&first.token).await.unwrap();
            // Advance the durable lease while A's already-entered scope is
            // paused. Leave its local cancellation signal undelivered.
            owners_b.heartbeat("manager-a", true).await.unwrap();
            owners_b.release(&first.token).await.unwrap();
            let successor = manager_b.acquire(id).await.unwrap().unwrap();
            assert!(successor.token.epoch > first.token.epoch);
            assert_eq!(successor.token.instance_id, "manager-b");
            owners_b.check(&successor.token).await.unwrap();
            assert_eq!(
                owners_a.check(&first.token).await,
                Err(OwnershipError::Stale)
            );
            assert_eq!(execution_owner().as_ref(), Some(&first.token));
            assert!(first.is_current());
            let result = if publish_stale {
                Some(
                    one.reject_command(
                        original_claim,
                        CommandFailureCode::RuntimeUnavailable,
                        decision_at,
                    )
                    .await,
                )
            } else {
                None
            };
            (successor, result)
        })
        .await
        .unwrap();

    let after_stale = two.get_agent(id).await.unwrap().unwrap();
    let after_stale_command = two.get_command(command_id).await.unwrap().unwrap();
    let runtime = Runtime::new(false, false);
    let operations = AgentOperationService::new(two.clone(), runtime.clone());
    let completed = successor
        .run(async {
            let retry = two
                .claim_next_command(CommandClaimRequest {
                    agent_id: id.clone(),
                    holder: "manager-b".into(),
                    now: claim_expiry,
                    expires_at: successor_expiry,
                })
                .await
                .unwrap();
            if let Some(retry) = retry {
                assert_eq!(retry.receipt, claimed.receipt);
                assert!(retry.work.lease.as_ref().unwrap().claim.fence > original_claim.fence);
                operations.execute_trigger_claim(&retry).await.unwrap();
                DurableAgentUpdateRecorder::new(two.clone())
                    .record_update(
                        id,
                        1,
                        &mut AgentMessageAssembly::default(),
                        layer3_adapter::ConversationUpdate::TurnEnded(
                            layer3_adapter::PromptResponse::new(
                                layer3_adapter::TurnStopReason::EndTurn,
                            ),
                        ),
                        layer2_core::now_rfc3339(),
                    )
                    .await
                    .unwrap();
                assert!(operations.execute_trigger_claim(&retry).await.is_err());
            }
            two.reconcile_command(command_id).await.unwrap()
        })
        .await
        .unwrap();
    let logs = two
        .agent_log(id, before.conversation.last_seq, 100)
        .await
        .unwrap();
    let sends = runtime.sends.load(Ordering::SeqCst);
    accepted.f.close().await;

    if let Some(result) = stale_result {
        assert!(
            result.is_err(),
            "old epoch {} committed after epoch {} took ownership: command={:?}, agent_seq={}->{}, successor_sends={sends}",
            first.token.epoch,
            successor.token.epoch,
            after_stale_command.status,
            before.conversation.last_seq,
            after_stale.conversation.last_seq,
        );
    }
    assert_eq!(after_stale, before);
    assert_eq!(after_stale_command, claimed);
    assert_eq!(completed.receipt, claimed.receipt);
    assert_eq!(completed.status, AgentResponseStatus::Completed);
    assert_eq!(sends, 1);
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnCompleted { .. }))
            .count(),
        1
    );
}

async fn register(store: Arc<dyn OwnershipStore>, id: &str) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store,
        Member {
            instance_id: id.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 1,
        },
    )
    .await
    .unwrap()
}
