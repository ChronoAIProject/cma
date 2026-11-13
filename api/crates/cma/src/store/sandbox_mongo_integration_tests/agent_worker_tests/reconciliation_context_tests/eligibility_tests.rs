use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn detached_ineligible_candidates_skip_reads_and_changes_recover_next_sweep() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let name = format!("cma_recovery_eligibility_{}", uuid::Uuid::new_v4().simple());
    let reads = Arc::new(AtomicU64::new(0));
    let captured = reads.clone();
    let observed = name.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.db == observed
                && event.command.get_str("find").ok() == Some(AGENTS)
                && event
                    .command
                    .get_document("filter")
                    .ok()
                    .is_some_and(|f| f.get_str("_id").is_ok())
            {
                captured.fetch_add(1, Ordering::SeqCst);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let mut paused = idle_connection_fixture(&store, "paused", AgentStatus::Paused).await;
    let mut closing = idle_connection_fixture(&store, "closing", AgentStatus::Ready).await;
    closing
        .work
        .schedule(AgentWorkStep::CloseProcess, NOW.into());
    closing.revision += 1;
    replace_agent_fixture(&store, &closing).await;
    let mut scheduled = idle_connection_fixture(&store, "scheduled", AgentStatus::Working).await;
    scheduled
        .work
        .schedule(AgentWorkStep::PublishConnection, NOW.into());
    scheduled.revision += 1;
    replace_agent_fixture(&store, &scheduled).await;
    let registry = Arc::new(LiveAgentRegistry::default());
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: Arc::new(CodexAdapter::with_backend(
                InMemoryHostedBackend::default(),
                CodexSandboxSpec::default(),
            )),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store.clone(), registry),
        },
        AgentWorkerConfig::new("eligibility", "eligibility"),
    );

    reads.store(0, Ordering::SeqCst);
    assert_eq!(
        worker
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "discovery already proves each no-op"
    );
    for original in [&paused, &closing, &scheduled] {
        assert_eq!(
            store.get_agent(&original.id).await.unwrap().as_ref(),
            Some(original)
        );
    }
    // No process-local cache: a resumed parent or finished lifecycle step can
    // recover immediately on the ordinary next sweep, through the existing CAS.
    paused.status = AgentStatus::Ready;
    paused.revision += 1;
    replace_agent_fixture(&store, &paused).await;
    closing.work.schedule(AgentWorkStep::None, NOW.into());
    closing.revision += 1;
    replace_agent_fixture(&store, &closing).await;
    reads.store(0, Ordering::SeqCst);
    assert_eq!(
        worker
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        reads.load(Ordering::SeqCst),
        4,
        "each recovered Agent reads its authority and carries matching integrity metadata"
    );
    for original in [&paused, &closing] {
        let current = store.get_agent(&original.id).await.unwrap().unwrap();
        assert_eq!(current.work.step, AgentWorkStep::PublishConnection);
        assert_eq!(current.conversation, original.conversation);
    }
    reads.store(0, Ordering::SeqCst);
    assert_eq!(
        worker
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        0
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.get_agent(&scheduled.id).await.unwrap(),
        Some(scheduled)
    );
    client.database(&name).drop().await.unwrap();
}
