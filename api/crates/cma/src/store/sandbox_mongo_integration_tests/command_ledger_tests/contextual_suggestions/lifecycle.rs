use super::*;

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn contextual_suggestions_mongo_admitted_input_invalidates_a_late_answer_before_new_turn_start(
) {
    let (client, name) = database("ticket13_admission").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let (parent, agent) = seed(
        &store,
        "admission",
        "Compare SQLite for local storage.",
        "SQLite suits a local-only app.",
    )
    .await;
    let model = Arc::new(HeldModel {
        calls: Default::default(),
        entered: Default::default(),
        release: Default::default(),
    });
    let worker = SuggestionGenerationService::new(store.clone());
    let active = model.clone();
    let task = tokio::spawn(async move { worker.run_next(active.as_ref()).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), model.entered.notified())
        .await
        .unwrap();
    let request = super::super::work_tests::live_request(&agent, &parent, "next-input");
    assert!(matches!(
        store.reserve_command(request).await.unwrap(),
        CommandReservationResult::Created(_)
    ));
    assert_eq!(
        store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .active_turn,
        None
    );
    assert_eq!(
        read(&router(store.clone()), &agent, "owner").await.1["status"],
        "expired"
    );
    model.release.notify_one();
    assert!(task.await.unwrap().unwrap());
    let replica = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let saved = read(&router(replica), &agent, "owner").await.1;
    assert_eq!(saved["status"], "expired");
    assert!(saved["set"].is_null());
    assert_eq!(saved["observation"]["usage"]["input_tokens"], 23);
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    client.database(&name).drop().await.unwrap();
}

struct HeldModel {
    calls: std::sync::atomic::AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl SuggestionGenerator for HeldModel {
    async fn generate(
        &self,
        _: &SuggestionWork,
    ) -> Result<SuggestionGeneration, SuggestionFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        Ok(SuggestionGeneration { output: r#"{"suggestions":[{"text":"Explain SQLite backup options.","evidence":"local-only app"}]}"#.into(),
            usage: SuggestionUsage { input_tokens: Some(23), ..Default::default() } })
    }
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn contextual_suggestions_mongo_crash_is_terminal_without_rebilling_on_a_second_replica() {
    let (client, name) = database("ticket13_restart").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let (_, agent) = seed(
        &store,
        "restart",
        "Compare SQLite for local storage.",
        "SQLite suits a local-only app.",
    )
    .await;
    let model = Arc::new(HeldModel {
        calls: Default::default(),
        entered: Default::default(),
        release: Default::default(),
    });
    let worker = SuggestionGenerationService::new(store.clone());
    let active = model.clone();
    let task = tokio::spawn(async move { worker.run_next(active.as_ref()).await });
    model.entered.notified().await;
    let replica = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    assert!(!SuggestionGenerationService::new(replica.clone())
        .run_next(model.as_ref())
        .await
        .unwrap());
    assert_eq!(
        read(&router(replica.clone()), &agent, "owner").await.1["status"],
        "running"
    );
    task.abort();
    let _ = task.await;
    assert!(replica
        .claim_suggestions(unix_millis() + 80_000)
        .await
        .unwrap()
        .is_none());
    let saved = read(&router(replica.clone()), &agent, "owner").await.1;
    assert_eq!(saved["status"], "failed");
    assert_eq!(saved["observation"]["failure"], "interrupted");
    assert!(saved["observation"]["usage"]["cost"].is_null());
    assert!(!SuggestionGenerationService::new(replica)
        .run_next(model.as_ref())
        .await
        .unwrap());
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn contextual_suggestions_mongo_late_result_remains_expired_after_scope_changes() {
    let (client, name) = database("ticket13_late").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let (_, agent) = seed(
        &store,
        "late",
        "Compare SQLite for local storage.",
        "SQLite suits a local-only app.",
    )
    .await;
    let model = Arc::new(HeldModel {
        calls: Default::default(),
        entered: Default::default(),
        release: Default::default(),
    });
    let worker = SuggestionGenerationService::new(store.clone());
    let active = model.clone();
    let task = tokio::spawn(async move { worker.run_next(active.as_ref()).await });
    model.entered.notified().await;
    let mut changed = store.get_agent(&agent.id).await.unwrap().unwrap();
    let revision = changed.revision;
    changed.conversation.runtime_generation = Some(2);
    changed.revision += 1;
    store.save_agent(revision, changed).await.unwrap();
    model.release.notify_one();
    assert!(task.await.unwrap().unwrap());
    let saved = read(&router(store.clone()), &agent, "owner").await.1;
    assert_eq!(saved["status"], "expired");
    assert!(saved["set"].is_null());
    assert_eq!(saved["observation"]["usage"]["input_tokens"], 23);
    let replica = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    assert_eq!(read(&router(replica), &agent, "owner").await.1, saved);
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    client.database(&name).drop().await.unwrap();
}
