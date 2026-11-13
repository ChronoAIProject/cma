//! Counter bindings through the actual command, turn and request transactions.
use super::*;
use layer2_core::budget::{PlatformPeriodLedger, PlatformSpendLedger, RequestBudget, RequestUsage};

async fn balances(store: &MongoSandboxStore) -> PlatformPeriodLedger {
    store
        .spend_state(wall_clock())
        .await
        .unwrap()
        .ledger
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn platform_counter_command_bindings_transfer_to_turns_and_release_without_dispatch() {
    for dispatch in [true, false] {
        let (client, name) = database("platform_counter_command").await;
        let (original, agent, parent, execution, _, sends) = composed(&client, &name).await;
        let store = Arc::new((*original).clone().with_platform_counter_shards(true));
        let policy = configure_metered(&store, &agent.owner).await;
        let admitted = record(
            store
                .reserve_command(live_request(&agent, &parent, "counter-command"))
                .await
                .unwrap(),
        );
        let command_key = format!("command:{}", admitted.receipt.envelope.command_id.as_str());
        let command = store
            .model_budget
            .find_one(doc! {"_id": command_key})
            .await
            .unwrap()
            .unwrap();
        let partition = command
            .get_document("record")
            .unwrap()
            .get("period_shard")
            .unwrap()
            .clone();
        assert_eq!(balances(&store).await.reserved, policy.turn_limit);
        if !dispatch {
            // A lower turn limit alone can run under the retained hold. A
            // lowered owner ceiling below outstanding liability must refuse.
            store.budget_settings.update_one(doc! {"_id": "platform"}, doc! {
                "$set": {
                    "settings.model_budget.owners.0.policy.turn_limit": (policy.turn_limit / 2) as i64,
                    "settings.model_budget.owners.0.policy.owner_day_limit": (policy.owner_day_limit / 2) as i64,
                },
                "$inc": {"revision": 1_i64},
            }).await.unwrap();
        }
        // The worker's store has the conversion switch OFF. Persisted bindings
        // still select the correct layout across rollout overlap and rollback.
        start_turn(&store, execution, &admitted).await;
        if dispatch {
            let working = store.get_agent(&agent.id).await.unwrap().unwrap();
            let turn = working.conversation.active_turn.unwrap();
            let record = store
                .model_budget
                .find_one(doc! {
                    "_id": format!("turn:{}:{turn}", agent.id.as_str()),
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                record.get_document("record").unwrap().get("period_shard"),
                Some(&partition)
            );
            assert_eq!(balances(&store).await.reserved, policy.turn_limit);
            finish_turn(&store, &agent).await;
            assert_eq!(sends.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(sends.load(Ordering::SeqCst), 0);
            let failed = store
                .get_command(&admitted.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(failed.status, AgentResponseStatus::Failed);
            assert!(failed.work.dispatch.is_none());
        }
        let final_balance = balances(&store).await;
        assert_eq!(final_balance.reserved, 0);
        assert_eq!(
            final_balance.spent,
            if dispatch { policy.turn_limit } else { 0 }
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn platform_counter_request_settlement_survives_restart_and_is_idempotent() {
    let (client, name) = database("platform_counter_request").await;
    let (original, scope, request) = super::super::request_budget::completed(&client, &name).await;
    meter(&original).await;
    let store = (*original).clone().with_platform_counter_shards(true);
    let hold = store
        .reserve_request(&scope, &request)
        .await
        .unwrap()
        .unwrap();
    let admitted = balances(&store).await;
    assert!(admitted.reserved > 0);
    let usage = RequestUsage {
        input_tokens: 7,
        output_tokens: 1,
    };
    let restart = MongoSandboxStore::open(&client, &name).await.unwrap();
    restart.settle_request(&hold, usage).await.unwrap();
    let settled = balances(&restart).await;
    assert_eq!(settled.reserved, 0);
    assert!(settled.spent > 0 && settled.spent < admitted.reserved);
    restart.settle_request(&hold, usage).await.unwrap();
    assert_eq!(balances(&restart).await, settled);
    assert_eq!(
        restart
            .settle_request(
                &hold,
                RequestUsage {
                    input_tokens: 8,
                    output_tokens: 1
                }
            )
            .await,
        Err(BudgetError::Unavailable)
    );
    assert_eq!(balances(&restart).await, settled);
    client.database(&name).drop().await.unwrap();
}
