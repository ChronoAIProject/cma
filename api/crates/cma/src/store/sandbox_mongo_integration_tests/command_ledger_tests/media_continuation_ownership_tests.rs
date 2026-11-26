use super::*;
use crate::store::agent_ownership::command_test_support::handoff;
use layer2_core::{
    agent_ownership::{Member, OwnershipCoordinator, PROTOCOL_VERSION},
    sandbox::{
        agent_io::{
            AgentCommandExecution, AgentCommandWorker, CommandAttempt, CommandPreparationError,
            CommandReadiness,
        },
        AgentMessageAssembly, DurableAgentUpdateRecorder,
    },
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_reconciliation_fences_the_same_transaction_after_handoff() {
    let (f, claimed) = deferred_fixture("media_owner_reconcile").await;
    let owners = handoff(&f.store, &f.agent.id).await;
    let id = &claimed.receipt.envelope.command_id;
    let before = owners.store.get_command(id).await.unwrap().unwrap();
    let agent_query = doc! {"_id": f.agent.id.as_str()};
    let row_before = owners
        .store
        .agents
        .find_one(agent_query.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        owners.store.reconcile_command(id).await,
        Err(CommandLedgerError::StaleClaim)
    ));
    assert!(matches!(
        owners
            .past
            .run(owners.store.reconcile_command(id))
            .await
            .unwrap(),
        Err(CommandLedgerError::Conflict)
    ));
    assert_eq!(owners.store.get_command(id).await.unwrap().unwrap(), before);
    assert_eq!(
        owners
            .store
            .agents
            .find_one(agent_query.clone())
            .await
            .unwrap()
            .unwrap(),
        row_before
    );
    let parent_query = doc! { "_id": f.agent.sandbox_id.as_str() };
    let parent_row = owners
        .store
        .sandboxes
        .find_one_and_delete(parent_query.clone())
        .await
        .unwrap()
        .unwrap();
    let db = f.client.database(&f.name);
    let bindings = db.collection::<mongodb::bson::Document>("provider_bindings");
    let binding = bindings.find_one(doc! {}).await.unwrap().unwrap();
    let command_query = doc! { "_id": id.as_str() };
    let command_row = owners
        .store
        .agent_commands
        .find_one(command_query.clone())
        .await
        .unwrap()
        .unwrap();
    let lane_row = owners
        .store
        .agent_command_orders
        .find_one(agent_query.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(before.work.dispatch.is_some());
    assert_eq!(
        owners
            .current
            .run(owners.store.reconcile_command(id))
            .await
            .unwrap(),
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(
        owners
            .store
            .sandboxes
            .find_one(parent_query.clone())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        owners
            .store
            .agent_commands
            .find_one(command_query)
            .await
            .unwrap(),
        Some(command_row)
    );
    assert_eq!(
        owners
            .store
            .agent_command_orders
            .find_one(agent_query.clone())
            .await
            .unwrap(),
        Some(lane_row)
    );
    assert_eq!(
        owners
            .store
            .agents
            .find_one(agent_query.clone())
            .await
            .unwrap()
            .as_ref(),
        Some(&row_before)
    );
    assert_eq!(bindings.find_one(doc! {}).await.unwrap(), Some(binding));
    owners.store.sandboxes.insert_one(parent_row).await.unwrap();
    let queued = owners
        .current
        .run(owners.store.reconcile_command(id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(queued.status, AgentResponseStatus::Queued);
    assert_eq!(queued.receipt, claimed.receipt);
    assert_eq!(queued.work.deferred_attempts.len(), 1);
    assert!(queued.work.dispatch.is_none());
    let row_after = owners
        .store
        .agents
        .find_one(agent_query.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row_after.get_i64("agent_command_admission_fence").unwrap(),
        row_before
            .get_i64("agent_command_admission_fence")
            .unwrap_or(0)
            + 1,
    );
    assert_eq!(owners.store.reconcile_command(id).await.unwrap(), queued);
    assert_eq!(
        owners
            .store
            .agents
            .find_one(agent_query)
            .await
            .unwrap()
            .unwrap(),
        row_after
    );
    f.close().await;
}

struct NoRedispatch;

#[async_trait::async_trait]
impl AgentCommandExecution for NoRedispatch {
    async fn check_queued(&self, _: &AdmittedAgentCommand) -> CommandReadiness {
        panic!("a persisted dispatch must use the ledger fold")
    }

    async fn prepare_queued(
        &self,
        _: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        panic!("a persisted dispatch must not request runtime preparation")
    }

    async fn execute(&self, _: &AdmittedAgentCommand) -> CommandAttempt {
        panic!("a persisted dispatch must not send native input again")
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_completed_fold_progresses_on_nonowner() {
    let (f, claimed) = claimed_continuation_fixture("media_ledger_fold").await;
    let authority = f
        .auth
        .authorize_resource_command(&f.access, &claimed.receipt.envelope)
        .await
        .unwrap()
        .with_source_guard(f.service.clone());
    let (runtime, sends) = runtime(&f, usize::MAX, false);
    AgentOperationService::new(f.store.clone(), runtime)
        .execute_resource_claim(&authority, &claimed)
        .await
        .unwrap();

    let id = &claimed.receipt.envelope.command_id;
    let generation = f.agent.conversation.runtime_generation.unwrap();
    DurableAgentUpdateRecorder::new(f.store.clone())
        .record_update(
            &f.agent.id,
            generation,
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let owners = handoff(&f.store, &f.agent.id).await;
    let peer = OwnershipCoordinator::register(
        owners.store.agent_ownership.clone().unwrap(),
        Member {
            instance_id: "ledger-peer".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://ledger-peer:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 16,
        },
    )
    .await
    .unwrap();
    assert!(peer.acquire(&f.agent.id).await.unwrap().is_none());
    let store = Arc::new(owners.store);
    let before = store.get_command(id).await.unwrap().unwrap();
    assert!(before.work.dispatch.is_some());
    assert!(
        !before.status.is_terminal(),
        "fixture must leave the command for the ledger worker"
    );
    let agent = store.get_agent(&f.agent.id).await.unwrap().unwrap();
    let worker =
        AgentCommandWorker::new(store.clone(), Arc::new(NoRedispatch), "ledger-peer".into())
            .unwrap()
            .with_ownership(Some(peer));

    assert!(worker.run_scheduled(id).await.unwrap().is_zero());
    let after = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(after.status, AgentResponseStatus::Completed);
    assert_eq!(after.receipt, before.receipt);
    assert_eq!(store.get_agent(&f.agent.id).await.unwrap().unwrap(), agent);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    f.close().await;
}
