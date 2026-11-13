use super::*;
use crate::store::sandbox_mongo_integration_tests::capacity_fixtures::fill_normal_capacity;
use layer3_adapter::{ConversationUpdate, ConversationUsage};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_refused_usage_settles_budget_and_public_response_exactly_once() {
    let (client, name) = database("capacity_usage_budget").await;
    let (store, agent, parent, execution, authority, sends) = composed_with_history(
        &client,
        &name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        active_sandbox("sbx_commands"),
        Some((100, true)),
    )
    .await;
    super::super::budget_tests::configure(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "usage-capacity"))
            .await
            .unwrap(),
    );
    let id = &admitted.receipt.envelope.command_id;
    let worker = AgentCommandWorker::new(store.clone(), execution, "usage-worker".into()).unwrap();
    worker.run_one(id).await.unwrap();
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let full = fill_normal_capacity(&store, working, false).await;
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let update = ConversationUpdate::Usage(ConversationUsage {
        turn_id: "measured-capacity-turn".into(),
        input_tokens: 80,
        output_tokens: 12,
        last_input_tokens: 80,
        last_output_tokens: 12,
        context_used: 4000,
        context_window: Some(8000),
        counters_reset: false,
    });
    let outcome = recorder
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            update.clone(),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(outcome.turn_closed);
    let saved = store.get_agent(&agent.id).await.unwrap().unwrap();
    let usage = saved.conversation.observations.usage.as_ref().unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (80, 12));
    assert_eq!(usage.context_used, 4000);
    let turn = full.conversation.active_turn.unwrap();
    let budget = store
        .model_budget
        .find_one(doc! {"_id":format!("turn:{}:{turn}",agent.id)})
        .await
        .unwrap()
        .unwrap();
    let reservation = budget
        .get_document("record")
        .unwrap()
        .get_document("reservation")
        .unwrap();
    assert!(reservation.get_bool("settled").unwrap());
    assert_eq!(reservation.get_i64("observed").unwrap(), 92);
    worker.run_one(id).await.unwrap();
    let response = super::capacity_public::response(
        store.clone(),
        &agent,
        authority,
        admitted.receipt.response_id.as_ref().unwrap(),
    )
    .await;
    assert_eq!(response["status"], "failed");
    assert_eq!(response["error"]["code"], "agent_log_capacity_exceeded");
    assert_eq!(response["usage"]["input_tokens"], 80);
    assert_eq!(response["usage"]["output_tokens"], 12);
    assert!(recorder
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            update,
            layer2_core::now_rfc3339()
        )
        .await
        .is_err());
    assert_eq!(
        store
            .model_budget
            .find_one(doc! {"_id":budget.get_str("_id").unwrap()})
            .await
            .unwrap()
            .unwrap(),
        budget
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let logs = store
        .agent_log(&agent.id, full.conversation.last_seq, 20)
        .await
        .unwrap();
    assert_eq!(
        logs.iter()
            .filter(|e| matches!(e.kind, AgentLogKind::UsageUpdated { .. }))
            .count(),
        1
    );
    assert_eq!(
        logs.iter()
            .filter(|e| matches!(e.kind, AgentLogKind::TurnFailed { .. }))
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
