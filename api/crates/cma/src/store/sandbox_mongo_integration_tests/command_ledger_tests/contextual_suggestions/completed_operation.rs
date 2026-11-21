use super::*;
use layer2_core::sandbox::{AgentOperationIntent, AgentOperationRecord};

struct CompletedModel;

#[async_trait::async_trait]
impl SuggestionGenerator for CompletedModel {
    async fn generate(
        &self,
        _: &SuggestionWork,
    ) -> Result<SuggestionGeneration, SuggestionFailure> {
        Ok(SuggestionGeneration {
            output: r#"{"suggestions":[{"text":"Explain SQLite backup options.","evidence":"local-only app"}]}"#.into(),
            usage: SuggestionUsage::default(),
        })
    }
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn contextual_suggestions_mongo_completed_message_retains_observed_receipt_and_generates_next_suggestion(
) {
    let (client, name) = database("ticket19_completed_operation").await;
    let store = Arc::new(enabled_store(&client, &name).await);
    let (parent, mut agent) = seed(
        &store,
        "completed-operation",
        "Compare SQLite for local storage.",
        "SQLite suits a local-only app.",
    )
    .await;
    let mut operation = AgentOperationRecord::new(
        2,
        1,
        3,
        AgentOperationIntent::send_message("Compare SQLite for local storage."),
        NOW.into(),
    )
    .unwrap();
    operation.mark_execution_observed(NOW.into()).unwrap();
    let previous = agent.revision;
    agent.revision += 1;
    agent.conversation.operation = Some(operation.clone());
    store.save_agent(previous, agent.clone()).await.unwrap();
    let routes = router(store.clone());
    assert_eq!(read(&routes, &agent, "owner").await.1["status"], "pending");
    assert!(SuggestionGenerationService::new(store.clone())
        .run_next(&CompletedModel)
        .await
        .unwrap());
    let reading = read(&routes, &agent, "owner").await.1;
    assert_eq!(reading["status"], "ready");
    assert_eq!(
        reading["set"]["items"][0]["text"],
        "Explain SQLite backup options."
    );
    assert_eq!(
        store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .operation,
        Some(operation.clone())
    );
    let request = super::super::work_tests::live_request(&agent, &parent, "later-input");
    assert!(matches!(
        store.reserve_command(request).await.unwrap(),
        CommandReservationResult::Created(_)
    ));
    let admitted = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(admitted.conversation.active_turn, None);
    assert_eq!(admitted.conversation.last_seq, agent.conversation.last_seq);
    assert_eq!(admitted.conversation.operation, Some(operation));
    assert_eq!(read(&routes, &agent, "owner").await.1["status"], "expired");
    client.database(&name).drop().await.unwrap();
}
