use super::super::budget_tests::configure_metered;
use super::*;
use layer2_core::budget::{BudgetPolicy, OwnerBudget};
use layer2_core::sandbox::store::SandboxReconciliationStore;

async fn running(
    client: &Client,
    name: &str,
) -> (Arc<MongoSandboxStore>, Agent, Sandbox, BudgetPolicy) {
    let (store, agent, parent, execution, _, sends) = composed(client, name).await;
    let policy = configure_metered(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "runtime-loss"))
            .await
            .unwrap(),
    );
    AgentCommandWorker::new(store.clone(), execution, "budget-recovery-worker".into())
        .unwrap()
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let agent = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert!(agent.conversation.active_turn.is_some());
    (store, agent, parent, policy)
}

fn loss(parent: &Sandbox) -> store::SandboxRuntimeLossRequest {
    store::SandboxRuntimeLossRequest {
        sandbox_id: parent.id.clone(),
        expected_revision: parent.revision,
        expected_runtime: parent.runtime.clone(),
        retain_runtime: false,
        native_retirement_finality: false,
        interruption_cause: Default::default(),
        observed_at: layer2_core::now_rfc3339(),
    }
}

async fn owner(store: &MongoSandboxStore, agent: &Agent) -> OwnerBudget {
    crate::store::sandbox_mongo::read_record(
        &store
            .model_budget
            .find_one(doc! {"_id": format!("owner:{}", agent.owner)})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_closes_real_admitted_turn_budget() {
    let (client, name) = database("budget_runtime_loss").await;
    let (store, agent, parent, policy) = running(&client, &name).await;
    assert_eq!(owner(&store, &agent).await.reserved, policy.turn_limit);
    assert_eq!(
        store.recover_runtime_loss(loss(&parent)).await.unwrap(),
        store::SandboxRuntimeLossOutcome::Failed
    );
    assert!(store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap()
        .conversation
        .active_turn
        .is_none());
    let charged = owner(&store, &agent).await;
    assert_eq!(
        charged.reserved, 0,
        "runtime loss must close the retained turn hold"
    );
    assert_eq!(charged.spent, policy.turn_limit);
    client.database(&name).drop().await.unwrap();
}

#[path = "runtime_loss_budget_atomicity.rs"]
mod atomicity;
#[path = "runtime_loss_budget_fixtures.rs"]
mod fixtures;
#[path = "runtime_loss_budget_recovery.rs"]
mod recovery;
