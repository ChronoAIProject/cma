use super::super::super::budget_tests::{
    configure_metered, open_platform, period_row, raise_owner_limit,
};
use super::*;
use crate::store::sandbox_mongo::read_record;
use layer2_core::budget::{OwnerBudget, TurnReservation};
use layer3_adapter::ConversationUsage;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ordinary_parent_retains_full_debit_and_refuses_auxiliary_requests_before_send() {
    let (client, name) = database("request_parent_ordinary").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    configure_metered(&store, &agent.owner).await;
    raise_owner_limit(&store, 64).await;
    // Give the real auxiliary quotes room to fit if the parent had spent only
    // the observed units. Neither the owner day nor platform period is binding.
    let turn_limit = 4096;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {
                "settings.model_budget.owners.0.policy.turn_limit": turn_limit as i64,
                "settings.model_budget.request_models": [
                    {"model": "budget-model", "input_overhead_tokens": 1_i64}
                ]
            }},
        )
        .await
        .unwrap();
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "parent-turn"))
            .await
            .unwrap(),
    );
    let command = &admitted.receipt.envelope.command_id;
    AgentCommandWorker::new(store.clone(), execution, "request-worker".into())
        .unwrap()
        .run_one(command)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let turn = working.conversation.active_turn.unwrap();
    let generation = working.conversation.runtime_generation.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let mut assembly = AgentMessageAssembly::default();
    for update in [
        ConversationUpdate::Usage(ConversationUsage {
            turn_id: "ordinary-provider-turn".into(),
            input_tokens: 7,
            output_tokens: 3,
            last_input_tokens: 7,
            last_output_tokens: 3,
            context_used: 10,
            context_window: None,
            counters_reset: false,
        }),
        ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
    ] {
        recorder
            .record_update(
                &agent.id,
                generation,
                &mut assembly,
                update,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        store.reconcile_command(command).await.unwrap().status,
        AgentResponseStatus::Completed
    );

    let turn_row = store
        .model_budget
        .find_one(doc! {"_id": format!("turn:{}:{turn}", agent.id)})
        .await
        .unwrap()
        .unwrap();
    let reservation: TurnReservation = mongodb::bson::deserialize_from_document(
        turn_row
            .get_document("record")
            .unwrap()
            .get_document("reservation")
            .unwrap()
            .clone(),
    )
    .unwrap();
    assert_eq!(reservation.observed, 10);
    assert!(reservation.settled);
    assert!(!reservation.final_usage_known);
    assert_eq!(reservation.final_model_calls, None);
    assert_eq!(reservation.settlement_debit(), turn_limit);

    let owner: OwnerBudget = read_record(
        &store
            .model_budget
            .find_one(doc! {"_id": format!("owner:{}", agent.owner)})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!((owner.reserved, owner.spent), (0, turn_limit));
    let period = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((period.reserved, period.spent), (0, turn_limit));

    crate::sandbox::assert_metered_auxiliary_refusal(
        store.clone(),
        RequestScope {
            agent_id: agent.id,
            turn_sequence: turn,
            request_id: "ordinary-title".into(),
        },
        BudgetError::TurnExhausted,
    )
    .await;
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {"_id": {"$regex": "^request:"}})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}
