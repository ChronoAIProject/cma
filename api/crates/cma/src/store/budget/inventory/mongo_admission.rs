use super::*;
use crate::store::sandbox_mongo_integration_tests::command_ledger_tests::{
    execution_tests::composed_with_input, work_tests::live_request,
};
use layer2_core::budget::{BudgetSettings, PlatformBudgetPolicy};
use layer2_core::sandbox::{
    agent_io::{AgentCommandStore, AgentCommandWorker},
    store::AgentStore,
    AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{ConversationInputSender, ConversationUpdate, PromptResponse, TurnStopReason};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_admission_transfer_and_settlement_are_not_counted_twice() {
    let fixture = Fixture::new().await;
    let input = ConversationInputSender::spawn(4, |_| async { Ok(()) });
    let (store, mut agent, parent, execution, _) = composed_with_input(
        &fixture.admin,
        fixture.database.name(),
        input,
        "inventory-native-conversation".into(),
    )
    .await;
    agent.capabilities = layer2_core::RuntimeCapabilities::new([
        layer2_core::RuntimeCapability::Conversation,
        layer2_core::RuntimeCapability::Usage,
        layer2_core::RuntimeCapability::Interrupt,
    ]);
    store
        .agents
        .replace_one(
            doc! {"_id": agent.id.as_str()},
            crate::store::agents::agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    let settings = BudgetSettings {
        enabled: true,
        default_policy: Some(saved(&agent).reservation.policy),
        platform: Some(PlatformBudgetPolicy {
            period_cap: 1_000_000,
            period_start: "2026-01-01T00:00:00Z".into(),
            period_end: "2100-01-01T00:00:00Z".into(),
            warning_percent: 80,
            stop_percent: 100,
            protected_reserve: 0,
            lag_allowance: 0,
            cloud: None,
        }),
        ..Default::default()
    };
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {
                "settings.model_budget": mongodb::bson::serialize_to_bson(&settings).unwrap()
            }},
        )
        .await
        .unwrap();
    let request = live_request(&agent, &parent, "inventory-real-admission");
    let command = request.envelope.command_id.clone();
    store.reserve_command(request).await.unwrap();
    AgentCommandWorker::new(store.clone(), execution, "inventory-worker".into())
        .unwrap()
        .run_one(&command)
        .await
        .unwrap();
    let active = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert!(active.conversation.active_turn.is_some());
    let report = fixture.observe(Limits::default(), &()).await.unwrap();
    assert_eq!(report.selected, 1);
    assert_eq!(report.settled, 0);
    assert_eq!(report.outcomes["matching_active_turn"], 1);
    DurableAgentUpdateRecorder::new(store.clone())
        .record_update(
            &agent.id,
            active.conversation.runtime_generation.unwrap(),
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let command_copy = fixture
        .budget()
        .find_one(doc! {"_id": format!("command:{command}")})
        .await
        .unwrap()
        .unwrap();
    let copy: super::super::super::TurnRecord =
        crate::store::sandbox_mongo::read_record(&command_copy).unwrap();
    assert!(!copy.reservation.settled);
    let report = fixture.observe(Limits::default(), &()).await.unwrap();
    assert_eq!(report.selected, 1);
    assert_eq!(report.settled, 1);
    assert!(report.outcomes.is_empty());
    fixture.assert_reads();
    fixture.cleanup().await;
}
