use super::*;
use crate::store::agent_ownership::MongoAgentOwnership;
use layer2_core::agent_ownership::{
    Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
};
use std::sync::Arc;
use work_tests::{claim_request, live_request, message_mutation};

async fn coordinator(store: Arc<MongoAgentOwnership>, id: &str) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store,
        Member {
            instance_id: id.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: format!("http://{id}:8080"),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn command_worker_transactions_fence_stale_owner_before_reconcile_or_reject() {
    let (client, name) = database("command_worker_owners").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "bound"))
            .await
            .unwrap(),
    );
    let claimed = store
        .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    store
        .commit_agent_conversation(message_mutation(
            &agent,
            &parent,
            &claimed.work.lease.as_ref().unwrap().claim,
        ))
        .await
        .unwrap();
    layer2_core::sandbox::DurableAgentUpdateRecorder::new(Arc::new(store.clone()))
        .record_update(
            &agent.id,
            1,
            &mut layer2_core::sandbox::AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let id = &admitted.receipt.envelope.command_id;
    let before = store.get_command(id).await.unwrap().unwrap();
    let owners = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
    let one = store
        .clone()
        .enable_agent_ownership("owner".into())
        .await
        .unwrap();
    let two = store
        .enable_agent_ownership("successor".into())
        .await
        .unwrap();
    let a = coordinator(owners.clone(), "owner").await;
    let old = a.acquire(&agent.id).await.unwrap().unwrap();
    let b = coordinator(owners.clone(), "successor").await;
    assert_eq!(
        one.reconcile_command(id).await,
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(one.get_command(id).await.unwrap(), Some(before.clone()));
    // Durable ownership moves while the old local Authority is still current:
    // cancellation alone cannot make this test pass.
    owners.heartbeat("owner", true).await.unwrap();
    owners.release(&old.token).await.unwrap();
    let next = b.acquire(&agent.id).await.unwrap().unwrap();
    assert!(old.is_current());
    assert!(next.token.epoch > old.token.epoch);
    assert_eq!(
        old.run(one.reconcile_command(id)).await.unwrap(),
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(one.get_command(id).await.unwrap(), Some(before.clone()));
    assert_eq!(
        next.run(one.reconcile_command(id)).await.unwrap(),
        Err(CommandLedgerError::Conflict),
        "another AM cannot borrow the successor token"
    );
    let two = Arc::new(two);
    let worker = AgentCommandWorker::new(
        two.clone(),
        Arc::new(crate::commands::ownership::OwnedCommandExecution {
            inner: Arc::new(ReconcileOnly),
            ownership: b,
        }),
        "successor-worker".into(),
    )
    .unwrap();
    worker.run_scheduled(id).await.unwrap();
    let done = two.get_command(id).await.unwrap().unwrap();
    assert_eq!(done.status, AgentResponseStatus::Completed);
    assert_eq!(done.receipt, admitted.receipt);
    assert_eq!(done.work.dispatch, before.work.dispatch);
    let current_agent = two.get_agent(&agent.id).await.unwrap().unwrap();
    let queued = record(
        two.reserve_command(live_request(&current_agent, &parent, "refused"))
            .await
            .unwrap(),
    );
    let reject = || {
        one.reject_unclaimed_command(
            &queued.receipt.envelope.command_id,
            queued.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
    };
    assert_eq!(
        old.run(reject()).await.unwrap(),
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(
        two.get_command(&queued.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(queued.clone())
    );
    let claim_request = claim_request(
        &current_agent,
        "refusal-worker",
        &layer2_core::now_rfc3339(),
    );
    assert_eq!(
        old.run(one.claim_observed_command(
            claim_request.clone(),
            &queued.receipt.envelope.command_id,
            queued.revision
        ))
        .await
        .unwrap(),
        Err(CommandLedgerError::Conflict)
    );
    let leased = next
        .run(two.claim_observed_command(
            claim_request,
            &queued.receipt.envelope.command_id,
            queued.revision,
        ))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let claim = &leased.work.lease.as_ref().unwrap().claim;
    assert_eq!(
        old.run(one.reject_command(
            claim,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339()
        ))
        .await
        .unwrap(),
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(
        two.get_command(&queued.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(leased.clone())
    );
    next.run(two.reject_command(
        claim,
        CommandFailureCode::AuthorityRevoked,
        layer2_core::now_rfc3339(),
    ))
    .await
    .unwrap()
    .unwrap();
    let refused = two
        .get_command(&queued.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refused.receipt, queued.receipt);
    assert_eq!(refused.status, AgentResponseStatus::Failed);
    let unclaimed = record(
        two.reserve_command(live_request(&current_agent, &parent, "unclaimed-refusal"))
            .await
            .unwrap(),
    );
    next.run(two.reject_unclaimed_command(
        &unclaimed.receipt.envelope.command_id,
        unclaimed.revision,
        CommandFailureCode::AuthorityRevoked,
        layer2_core::now_rfc3339(),
    ))
    .await
    .unwrap()
    .unwrap();
    let refused = two
        .get_command(&unclaimed.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refused.receipt, unclaimed.receipt);
    assert_eq!(refused.status, AgentResponseStatus::Failed);
    client.database(&name).drop().await.unwrap();
}

struct ReconcileOnly;
#[async_trait::async_trait]
impl AgentCommandExecution for ReconcileOnly {
    async fn check_queued(&self, _: &AdmittedAgentCommand) -> CommandReadiness {
        panic!("bound command must bypass readiness")
    }
    async fn prepare_queued(
        &self,
        _: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        panic!("bound command must not prepare again")
    }
    async fn execute(&self, _: &AdmittedAgentCommand) -> CommandAttempt {
        panic!("bound command must never replay native input")
    }
}
