use super::*;

struct NoGeneration;
#[async_trait::async_trait]
impl SuggestionGenerator for NoGeneration {
    async fn generate(
        &self,
        _: &SuggestionWork,
    ) -> Result<SuggestionGeneration, SuggestionFailure> {
        panic!("closed or unselected writers must not generate suggestions");
    }
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn contextual_suggestions_mongo_writer_admission_preserves_pending_work_and_reads() {
    let (client, name) = database("writer_admission").await;
    let closed = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let (_, old_agent) = seed(
        &closed,
        "closed",
        "Compare SQLite for local storage.",
        "SQLite suits a local-only app.",
    )
    .await;
    assert_eq!(
        closed.suggestions.count_documents(doc! {}).await.unwrap(),
        0
    );
    assert!(!SuggestionGenerationService::new(closed.clone())
        .run_next(&NoGeneration)
        .await
        .unwrap());
    assert_eq!(
        read(&router(closed.clone()), &old_agent, "owner").await.1["status"],
        "unavailable"
    );
    let admitted = Arc::new(enabled_store(&client, &name).await);
    let (_, agent) = seed(
        &admitted,
        "admitted",
        "Compare SQLite for local storage.",
        "SQLite suits a local-only app.",
    )
    .await;
    let before = admitted
        .suggestions
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let mut unselected = MongoSandboxStore::open(&client, &name).await.unwrap();
    unselected.writer_policy = Arc::new(serde_json::from_value(serde_json::json!({
        "mode":"accounts", "accounts":["another-owner"], "capabilities":["suggested_followup_admission"]
    })).unwrap());
    assert!(!SuggestionGenerationService::new(Arc::new(unselected))
        .run_next(&NoGeneration)
        .await
        .unwrap());
    assert!(!SuggestionGenerationService::new(closed.clone())
        .run_next(&NoGeneration)
        .await
        .unwrap());
    assert_eq!(
        closed.suggestions.find_one(doc! {}).await.unwrap().unwrap(),
        before
    );
    assert_eq!(
        read(&router(closed.clone()), &agent, "owner").await.1["status"],
        "pending"
    );
    let claim = admitted
        .claim_suggestions(unix_millis())
        .await
        .unwrap()
        .unwrap();
    // Already claimed work retains its exact settlement path after closure.
    closed
        .complete_suggestions(
            &claim,
            SuggestionCompletion {
                status: SuggestionStatus::Empty,
                set: None,
                observation: SuggestionObservation {
                    elapsed_ms: 1,
                    usage: SuggestionUsage::default(),
                    failure: None,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        read(&router(closed.clone()), &agent, "owner").await.1["status"],
        "empty"
    );
    client.database(&name).drop().await.unwrap();
}
