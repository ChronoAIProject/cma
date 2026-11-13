use super::*;
use crate::turns::routed::{HttpExecutionPort, Phase};

#[tokio::test]
async fn expired_prepared_media_is_inert_then_retired_without_posting_or_late_delivery() {
    let waiters = Arc::new(SyncRegistry::default());
    let (waiter_id, waiter) = waiters.register_with_media_responses(true).unwrap();
    let custom = CustomDelivery::new("https://gateway.example".into(), waiters);
    let mut activity = fixture::custom(None);
    let cmaeg_core::activity::ActivityKind::Event {
        attachments, ce, ..
    } = &mut activity.kind
    else {
        unreachable!()
    };
    ce.data = Some(json!({"audio":format!("data:audio/wav;base64,{}",STANDARD.encode(AUDIO))}));
    attachments.push(serde_json::from_value(json!({"reference":"audio","kind":"audio","filename":"audio.wav","duration_seconds":1,"source":{"type":"inline","pointer":"/audio"}})).unwrap());
    let mut request = fixture::request(
        |value| {
            value["provider"]["target"] =
                json!({"type":"existing_agent","agent_id":"agt_existing"});
            value["reply"]["sinks"] = json!([{"type":"sync"}]);
        },
        activity,
    );
    fixture::rewrite_ref(&mut request, |event| {
        event.reply = ReplyAddress::Sync { waiter: waiter_id }
    });
    let setup = Setup::with_request(supported(), request).await;
    let state = Manager::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(manager).with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let registry = crate::providers::ProviderRegistry::parse(&json!([{
        "slug":"cma_codex","base_url":base,"kind":"responses_http","max_inflight":2,
        "paths":{"agent_card":"/card","binding":"/api/v1/cmaeg/bindings/{binding_id}",
            "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}",
            "responses":"/api/v2/responses","recovery":{"response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"}}
    }]).to_string()).unwrap();
    let providers = Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 2)
            .unwrap()
            .with_cma_origin_fixture(&base),
    );
    let speech = Arc::new(HeldTranscript {
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
        release: Semaphore::new(0),
        completed: Notify::new(),
    });
    let media = inline_media_with(speech.clone());
    let builder = Arc::new(
        ProductionFrozenTurnBuilder::new(providers.clone(), Arc::new(fixture::keys()))
            .with_media(
                Some(media.clone()),
                PrepareConfig {
                    budget: Duration::from_millis(100),
                    ..PrepareConfig::default()
                },
            )
            .with_media_continuations(true),
    );
    let current = setup.current.channel.lock().unwrap().clone();
    let body = builder
        .prepare(&setup.plan, setup.target(), &current)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), speech.entered.notified())
        .await
        .unwrap();
    let index_server = crate::ingress::routing_store::test_server::Server::start().await;
    let index = MediaWorkIndex::fixture(
        setup.journal().clone(),
        setup.server.store(),
        index_server.records.clone(),
    );
    let mut lease = setup
        .journal()
        .claim(&setup.plan, setup.target(), "prepare-before-expiry")
        .await
        .unwrap();
    setup
        .journal()
        .freeze(&mut lease, &setup.plan, setup.target(), &body)
        .await
        .unwrap();
    index
        .prepare(&setup.plan, setup.target(), &body)
        .await
        .unwrap();
    setup.journal().release(&mut lease).await.unwrap();
    let store = MediaObligationStore(setup.journal().clone());
    // Retain a valid provider-side preparation too. It must never authorize a
    // supplement while the original Responses turn has not been admitted.
    store
        .initialize(
            &setup.plan,
            setup.target(),
            0,
            None,
            Some(MediaContinuationKind::Transcript),
        )
        .await
        .unwrap();
    let mut preparation = store
        .claim(&setup.plan, setup.target(), 0, "prepared-file")
        .await
        .unwrap();
    store
        .freeze_original(
            &mut preparation,
            MediaArtifactClaim {
                attachment_index: 0,
                kind: MediaContinuationKind::Original,
                content_sha256: sha256(AUDIO),
                source_sha256: sha256(AUDIO),
                name: "audio.wav".into(),
                media_type: "audio/x-wav".into(),
                size_bytes: AUDIO.len() as u64,
                text_sha256: None,
            },
        )
        .await
        .unwrap();
    store
        .record_upload(
            &mut preparation,
            MediaUploadReceipt {
                id: "att_audio".into(),
                agent_id: "agt_existing".into(),
                name: "audio.wav".into(),
                path: "/home/attachments/agt_existing/att_audio/audio.wav".into(),
                size_bytes: AUDIO.len() as u64,
                media_type: "audio/x-wav".into(),
                sha256: sha256(AUDIO),
            },
        )
        .await
        .unwrap();
    store.release(&mut preparation).await.unwrap();
    let (tx, mut rx) = mpsc::channel(1);
    let coordinator =
        DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx))).with_custom_stream(custom);
    let worker = MediaWorker {
        index: index.clone(),
        plans: setup.server.store(),
        store: store.clone(),
        authority: setup.current.clone(),
        providers: providers.clone(),
        media,
        delivery: Arc::new(MediaDelivery {
            store: store.clone(),
            authority: setup.current.clone(),
            providers: providers.clone(),
            coordinator: coordinator.clone(),
        }),
        worker: "expired-prepared-media".into(),
    };
    let work = WorkItem {
        identity: setup.plan.identity.clone(),
        target_id: setup.target().id.clone(),
    };
    let before = state.paths.lock().unwrap().clone();
    assert!(!worker.run(&work).await.unwrap());
    assert_eq!(
        *state.paths.lock().unwrap(),
        before,
        "Prepared cannot upload or append"
    );
    setup
        .server
        .memory
        .clock
        .store(setup.target().launch.event.expires_at, Ordering::SeqCst);
    let dispatcher = Arc::new(HttpExecutionPort::new(
        providers,
        builder,
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
    ))
    .dispatcher();
    for _ in 0..2 {
        assert!(
            matches!(dispatcher.dispatch(&work.identity,&work.target_id,"expired").await.unwrap(),DispatchOutcome::Terminal(Phase::Refused {code}) if code=="event_expired_before_start")
        );
    }
    RoutedDelivery::new(
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
        coordinator,
    )
    .report_expired_before_start(&work.identity, &work.target_id)
    .await
    .unwrap();
    // Before cleanup, the retained preparation's own effect guard is closed.
    let mut closed = store
        .claim(&setup.plan, setup.target(), 0, "prove-inert")
        .await
        .unwrap();
    assert!(matches!(
        store.guard_effect(&closed).await,
        Err(crate::turns::routed::ExecutionError::Expired)
    ));
    store.release(&mut closed).await.unwrap();
    assert!(worker.run(&work).await.unwrap());
    assert!(index
        .load(&setup.plan, setup.target())
        .await
        .unwrap()
        .is_none());
    let terminal = store
        .load(&setup.plan, setup.target(), 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(terminal.refusal.as_deref(), Some("event_expired"));
    assert!(terminal.supplements.is_empty());
    speech.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), speech.completed.notified())
        .await
        .unwrap();
    // Restarted discovery sees no active head. A late derivative cannot recreate
    // it, and another executor pass still cannot reach the provider or recipient.
    let restarted = MediaWorkIndex::fixture(
        setup.journal().clone(),
        setup.server.store(),
        index_server.records.clone(),
    );
    assert!(restarted
        .load(&setup.plan, setup.target())
        .await
        .unwrap()
        .is_none());
    assert!(worker.run(&work).await.unwrap());
    assert_eq!(*state.paths.lock().unwrap(), before);
    assert!(state.supplements.lock().unwrap().is_empty());
    assert!(rx.try_recv().is_err());
    let bytes = tokio::time::timeout(
        Duration::from_secs(2),
        to_bytes(Sse::new(waiter.stream()).into_response().into_body(), 65536),
    )
    .await
    .unwrap()
    .unwrap();
    let stream = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!stream.contains(WORDS));
    assert!(!stream.contains("event: cmaeg.media.response"));
    assert!(!stream.contains("event: cmaeg.completed"));
    assert!(stream.contains("event_expired_before_start"));
    server.abort();
}
