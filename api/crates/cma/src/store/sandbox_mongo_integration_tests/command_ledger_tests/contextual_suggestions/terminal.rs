use super::*;
use crate::store::sandbox_mongo::read_record;

async fn past_deadline(store: &MongoSandboxStore, agent: &Agent) -> SuggestionWork {
    let row = store
        .suggestions
        .find_one(doc! {"agent_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut pending: SuggestionWork = read_record(&row).unwrap();
    let started = unix_millis() - 71_000;
    pending.created_at_ms = started;
    store
        .suggestions
        .replace_one(
            doc! {"_id": &pending.id},
            crate::store::suggestions::document(&pending).unwrap(),
        )
        .await
        .unwrap();
    let claim = store.claim_suggestions(started).await.unwrap().unwrap();
    assert!(claim.deadline_ms.unwrap() < unix_millis());
    claim
}

fn ready(work: &SuggestionWork) -> SuggestionCompletion {
    SuggestionCompletion {
        status: SuggestionStatus::Ready,
        set: Some(SuggestedReplySet {
            id: work.id.clone(),
            viewer_id: work.owner.to_string(),
            agent_id: work.agent_id.to_string(),
            answer_id: format!("event:{}", work.scope.answer_sequence),
            scope: work.scope,
            source: SuggestionSource {
                kind: "generated".into(),
                version: work.strategy_version.clone(),
            },
            items: vec![SuggestedReply {
                id: format!("{}:0", work.id),
                text: "Explain SQLite backup options.".into(),
                rank: 0,
            }],
        }),
        observation: SuggestionObservation {
            elapsed_ms: 71_000,
            usage: SuggestionUsage::default(),
            failure: None,
        },
    }
}

async fn stored(store: &MongoSandboxStore, work: &SuggestionWork) -> Document {
    store
        .suggestions
        .find_one(doc! {"_id": &work.id})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn overdue_running_read_remains_observable_until_delayed_completion() {
    let (client, name) = database("suggestion_overdue_read").await;
    let store = Arc::new(enabled_store(&client, &name).await);
    let (_, agent) = seed(
        &store,
        "overdue",
        "Compare local storage options.",
        "SQLite suits a local-only app.",
    )
    .await;
    let work = past_deadline(&store, &agent).await;
    let before = stored(&store, &work).await;
    let routes = router(store.clone());
    let (status, reading) = read(&routes, &agent, "owner").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reading["status"], "running");
    assert!(reading["set"].is_null());
    assert!(reading["observation"].is_null());
    assert_eq!(stored(&store, &work).await, before);

    let replica = Arc::new(enabled_store(&client, &name).await);
    replica
        .complete_suggestions(&work, ready(&work))
        .await
        .unwrap();
    let completed = read(&routes, &agent, "owner").await.1;
    assert_eq!(completed["status"], "ready");
    assert_eq!(completed["set"]["scope"], serde_json::json!(work.scope));
    assert_eq!(
        completed["set"]["items"][0]["text"],
        "Explain SQLite backup options."
    );
    assert!(replica
        .claim_suggestions(unix_millis())
        .await
        .unwrap()
        .is_none());
    assert_eq!(read(&router(replica), &agent, "owner").await.1, completed);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn persisted_sweeper_failure_refuses_a_late_ready_completion() {
    let (client, name) = database("suggestion_swept_terminal").await;
    let store = Arc::new(enabled_store(&client, &name).await);
    let (_, agent) = seed(
        &store,
        "swept",
        "Compare local storage options.",
        "SQLite suits a local-only app.",
    )
    .await;
    let work = past_deadline(&store, &agent).await;
    let replica = Arc::new(enabled_store(&client, &name).await);
    assert!(replica
        .claim_suggestions(unix_millis())
        .await
        .unwrap()
        .is_none());
    let before = stored(&store, &work).await;
    let failed = read(&router(store.clone()), &agent, "owner").await.1;
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["observation"]["failure"], "interrupted");
    assert!(failed["set"].is_null());
    store
        .complete_suggestions(&work, ready(&work))
        .await
        .unwrap();
    assert_eq!(stored(&store, &work).await, before);
    assert_eq!(read(&router(replica), &agent, "owner").await.1, failed);
    client.database(&name).drop().await.unwrap();
}

struct FailingGenerator(Mutex<Option<SuggestionWork>>);

#[async_trait::async_trait]
impl SuggestionGenerator for FailingGenerator {
    async fn generate(
        &self,
        work: &SuggestionWork,
    ) -> Result<SuggestionGeneration, SuggestionFailure> {
        *self.0.lock().unwrap() = Some(work.clone());
        Err(SuggestionFailure::Unavailable)
    }
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn persisted_generator_failure_refuses_a_late_ready_completion() {
    let (client, name) = database("suggestion_generator_terminal").await;
    let store = Arc::new(enabled_store(&client, &name).await);
    let (_, agent) = seed(
        &store,
        "failed",
        "Compare local storage options.",
        "SQLite suits a local-only app.",
    )
    .await;
    let generator = FailingGenerator(Mutex::new(None));
    assert!(SuggestionGenerationService::new(store.clone())
        .run_next(&generator)
        .await
        .unwrap());
    let work = generator.0.lock().unwrap().clone().unwrap();
    let before = stored(&store, &work).await;
    let routes = router(store.clone());
    let failed = read(&routes, &agent, "owner").await.1;
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["observation"]["failure"], "unavailable");
    assert!(failed["set"].is_null());
    let replica = Arc::new(enabled_store(&client, &name).await);
    replica
        .complete_suggestions(&work, ready(&work))
        .await
        .unwrap();
    assert_eq!(stored(&store, &work).await, before);
    assert_eq!(read(&routes, &agent, "owner").await.1, failed);
    assert!(replica
        .claim_suggestions(unix_millis())
        .await
        .unwrap()
        .is_none());
    client.database(&name).drop().await.unwrap();
}
