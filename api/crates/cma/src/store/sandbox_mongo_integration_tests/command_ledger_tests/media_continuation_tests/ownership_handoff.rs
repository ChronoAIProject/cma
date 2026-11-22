use super::*;
use layer2_core::agent_ownership::{
    execution_owner, Member, OwnershipCoordinator, OwnershipError, OwnershipStore, PROTOCOL_VERSION,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn stale_manager_cannot_requeue_deferred_continuation_after_handoff() {
    Box::pin(run_handoff(true)).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn successor_reconciles_deferred_continuation_and_sends_once() {
    Box::pin(run_handoff(false)).await;
}

async fn run_handoff(reconcile_stale: bool) {
    let (f, claimed) = deferred_fixture("media_retry_owner_handoff").await;
    let agent_id = &f.agent.id;
    let id = &claimed.receipt.envelope.command_id;
    let peer = Client::with_uri_str(
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("use disposable Mongo"),
    )
    .await
    .unwrap();
    let one = open_replica(&f.client, &f.name, "manager-a").await;
    let two = open_replica(&peer, &f.name, "manager-b").await;
    let owners_a = one.agent_ownership.as_ref().unwrap();
    let owners_b = two.agent_ownership.as_ref().unwrap();
    let manager_a = register(owners_a.clone(), "manager-a").await;
    let first = manager_a.acquire(agent_id).await.unwrap().unwrap();
    let manager_b = register(owners_b.clone(), "manager-b").await;
    assert!(manager_b.acquire(agent_id).await.unwrap().is_none());

    let before = snapshot(&two, agent_id, id).await;
    let original_dispatch = before.command.work.dispatch.as_ref().unwrap();
    assert_eq!(
        original_dispatch.claim,
        claimed.work.lease.as_ref().unwrap().claim
    );
    assert_eq!(before.command.status, AgentResponseStatus::InProgress);
    assert!(before.command.work.deferred_attempts.is_empty());
    assert!(f.peer.outage.load(Ordering::SeqCst));

    let (successor, stale_result) = first
        .run(async {
            owners_a.check(&first.token).await.unwrap();
            // Move the durable lease while A is paused in its original scope.
            // A has yet to observe local cancellation.
            owners_b.heartbeat("manager-a", true).await.unwrap();
            owners_b.release(&first.token).await.unwrap();
            let successor = manager_b.acquire(agent_id).await.unwrap().unwrap();
            assert!(successor.token.epoch > first.token.epoch);
            owners_b.check(&successor.token).await.unwrap();
            assert_eq!(
                owners_a.check(&first.token).await,
                Err(OwnershipError::Stale)
            );
            assert_eq!(execution_owner().as_ref(), Some(&first.token));
            assert!(first.is_current());
            let result = if reconcile_stale {
                Some(one.reconcile_command(id).await)
            } else {
                None
            };
            (successor, result)
        })
        .await
        .unwrap();

    let after_stale = snapshot(&two, agent_id, id).await;
    let (runtime, sends) = runtime(&f, usize::MAX, false);
    let operations = AgentOperationService::new(two.clone(), runtime);
    let (queued, after_requeue, completed) = successor
        .run(async {
            let queued = two.reconcile_command(id).await.unwrap();
            assert_eq!(queued.status, AgentResponseStatus::Queued);
            assert_eq!(queued.receipt, before.command.receipt);
            assert!(queued.work.dispatch.is_none());
            assert_eq!(queued.work.deferred_attempts.len(), 1);
            assert_eq!(
                &queued.work.deferred_attempts[0].dispatch,
                original_dispatch
            );
            assert_eq!(two.reconcile_command(id).await.unwrap(), queued);
            let after_requeue = snapshot(&two, agent_id, id).await;

            f.peer.outage.store(false, Ordering::SeqCst);
            tokio::time::sleep(queued.continuation_retry_delay(&layer2_core::now_rfc3339())).await;
            let retry = two
                .claim_next_command(claim_request(
                    &f.agent,
                    "manager-b",
                    &layer2_core::now_rfc3339(),
                ))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(retry.receipt, queued.receipt);
            assert!(retry.work.lease.as_ref().unwrap().claim.fence > original_dispatch.claim.fence);
            let authority = f
                .auth
                .authorize_resource_command(&f.access, &retry.receipt.envelope)
                .await
                .unwrap()
                .with_source_guard(f.service.clone());
            operations
                .execute_resource_claim(&authority, &retry)
                .await
                .unwrap();
            assert!(operations
                .execute_resource_claim(&authority, &retry)
                .await
                .is_err());
            DurableAgentUpdateRecorder::new(two.clone())
                .record_update(
                    agent_id,
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
            let completed = two.reconcile_command(id).await.unwrap();
            assert_eq!(two.reconcile_command(id).await.unwrap(), completed);
            (queued, after_requeue, completed)
        })
        .await
        .unwrap();
    f.close().await;

    if let Some(result) = stale_result {
        assert!(
            result.as_ref().is_err() || result.as_ref().is_ok_and(|command| command == &before.command),
            "old epoch {} requeued after epoch {} took ownership: command={:?}, deferred_attempts={}, admission_fence={}->{}, lane_changed={}",
            first.token.epoch,
            successor.token.epoch,
            after_stale.command.status,
            after_stale.command.work.deferred_attempts.len(),
            before.admission_fence(),
            after_stale.admission_fence(),
            before.lane != after_stale.lane,
        );
    }
    assert_eq!(after_stale, before);
    assert_eq!(
        after_requeue.admission_fence(),
        before.admission_fence() + 1
    );
    assert_eq!(completed.receipt, queued.receipt);
    assert_eq!(
        completed.work.deferred_attempts,
        queued.work.deferred_attempts
    );
    assert_eq!(completed.status, AgentResponseStatus::Completed);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    agent: Document,
    lane: Document,
    command: AdmittedAgentCommand,
}

impl Snapshot {
    fn admission_fence(&self) -> i64 {
        self.agent
            .get_i64("agent_command_admission_fence")
            .unwrap_or(0)
    }
}

async fn snapshot(store: &MongoSandboxStore, agent_id: &AgentId, id: &AgentCommandId) -> Snapshot {
    let filter = doc! {"_id": agent_id.as_str()};
    Snapshot {
        agent: store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap(),
        lane: store
            .agent_command_orders
            .find_one(filter)
            .await
            .unwrap()
            .unwrap(),
        command: store.get_command(id).await.unwrap().unwrap(),
    }
}

async fn open_replica(client: &Client, database: &str, name: &str) -> Arc<MongoSandboxStore> {
    Arc::new(
        MongoSandboxStore::open(client, database)
            .await
            .unwrap()
            .enable_agent_ownership(name.into())
            .await
            .unwrap(),
    )
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
