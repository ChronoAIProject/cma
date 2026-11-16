use super::*;

// The real own-Pod reader must distinguish a planned deletion from a lost
// process identity. The serving loop listens to this exact failure watch.
#[tokio::test]
async fn planned_deletion_does_not_signal_process_failure() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::prepare(
        store.client(),
        Member {
            instance_id: "planned-deletion".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 4,
        },
    );
    let (_, health) = crate::health::ownership_fixture(Arc::new(StoreProbe), &coordinator, None);
    tokio::task::yield_now().await;
    let reply = Arc::new(Mutex::new(pod(true)));
    let probe = AdmissionProbe::install(
        &coordinator,
        Some(reader(reply.clone(), Arc::new(AtomicUsize::new(0)))),
        health,
        Termination::observe(std::future::pending()),
    )
    .unwrap();
    assert!(probe.check(&coordinator).await);
    let authority = coordinator
        .acquire(&AgentId("planned-agent".into()))
        .await
        .unwrap()
        .unwrap();
    reply.lock().unwrap()["metadata"]["deletionTimestamp"] = "2026-11-16T00:00:00Z".into();
    assert!(!probe.check(&coordinator).await);
    assert!(!coordinator.can_acquire());
    assert!(
        !*coordinator.failure_signal().borrow(),
        "planned deletion must not terminate HTTP before the preStop helper"
    );
    let retained = coordinator.begin_drain().await;
    assert!(!authority.is_current());
    assert_eq!(retained.len(), 1);
    assert!(store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_some());
    coordinator
        .handoff(&retained[0], async { Ok(()) })
        .await
        .unwrap();
    assert!(!*coordinator.failure_signal().borrow());
    assert!(store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(!authority.is_current());
}

#[tokio::test]
async fn replaced_container_still_signals_process_failure() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::prepare(
        store.client(),
        Member {
            instance_id: "replaced-manager".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 4,
        },
    );
    let (_, health) = crate::health::ownership_fixture(Arc::new(StoreProbe), &coordinator, None);
    tokio::task::yield_now().await;
    let reply = Arc::new(Mutex::new(pod(true)));
    let probe = AdmissionProbe::install(
        &coordinator,
        Some(reader(reply.clone(), Arc::new(AtomicUsize::new(0)))),
        health,
        Termination::observe(std::future::pending()),
    )
    .unwrap();
    assert!(probe.check(&coordinator).await);
    let authority = coordinator
        .acquire(&AgentId("replacement-agent".into()))
        .await
        .unwrap()
        .unwrap();
    reply.lock().unwrap()["status"]["containerStatuses"][0]["containerID"] =
        "containerd://replacement".into();
    assert!(!probe.check(&coordinator).await);
    assert!(*coordinator.failure_signal().borrow());
    assert!(!authority.is_current());
    assert!(store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_some());
}

struct DeletionReceiver {
    entered: tokio::sync::Notify,
    acknowledgement: tokio::sync::Notify,
    refuse: bool,
}

#[async_trait::async_trait]
impl layer3_adapter::owner_protocol::ReceiverRevocation for DeletionReceiver {
    fn identity(&self) -> String {
        "planned-deletion-native-receiver".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        self.entered.notify_one();
        self.acknowledgement.notified().await;
        if self.refuse {
            Err(layer3_adapter::AdapterError::Unavailable(
                "synthetic receiver refusal".into(),
            ))
        } else {
            Ok(())
        }
    }
}

struct DeletionServer(tokio::task::JoinHandle<std::io::Result<()>>);
impl Drop for DeletionServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct ReadinessWorker(tokio::task::JoinHandle<()>);
impl Drop for ReadinessWorker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn mongo_planned_deletion(refuse: bool) {
    use futures_util::FutureExt;
    use layer3_adapter::owner_protocol::TransportAuthority;
    use std::panic::AssertUnwindSafe;
    let (mongo, database, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    let result = AssertUnwindSafe(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let manager = AgentManager::prepare(
            Arc::new(durable.enable_agent_ownership("planned-mongo-manager".into()).await.unwrap()),
            Arc::new(LiveAgentRegistry::default()), address.clone(), 4,
        ).await.unwrap();
        manager.register_lifecycle(crate::agent_lifecycle::LifecycleDrain::fixture(1)).unwrap();
        manager.activate().unwrap();
        let (router, health) = crate::health::ownership_fixture(Arc::new(StoreProbe), &manager.coordinator, None);
        tokio::task::yield_now().await;
        let reply = Arc::new(Mutex::new(pod(true)));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let termination = Termination::observe(async { let _ = stopped.await; });
        let probe = AdmissionProbe::install(&manager.coordinator,
            Some(reader(reply.clone(), Arc::new(AtomicUsize::new(0)))), health, termination.clone()).unwrap();
        let mut observer = ReadinessWorker(manager.spawn_readiness_probe(probe, termination.clone()));
        tokio::time::timeout(Duration::from_secs(3), async {
            while !manager.coordinator.can_acquire() { tokio::task::yield_now().await; }
        }).await.unwrap();
        let authority = manager.coordinator.acquire(&AgentId("mongo-deletion-agent".into())).await.unwrap().unwrap();
        let original = manager.coordinator.store.owner(&authority.token.agent_id).await.unwrap().unwrap();
        let receiver = Arc::new(DeletionReceiver {
            entered: tokio::sync::Notify::new(), acknowledgement: tokio::sync::Notify::new(), refuse,
        });
        authority.register_receiver(receiver.clone()).unwrap();
        // Use the same real manager failure/termination signals that drive startup.
        let running_manager = manager.clone();
        let signal = termination.clone();
        let server = DeletionServer(tokio::spawn(async move {
            axum::serve(listener, crate::with_request_context(router))
                .with_graceful_shutdown(async move {
                    tokio::select! { _ = running_manager.failed() => (), _ = signal.cancelled() => () }
                }).await
        }));
        let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(2)).build().unwrap();
        assert_eq!(client.get(format!("{address}/ready")).send().await.unwrap().status(), StatusCode::OK);
        reply.lock().unwrap()["metadata"]["deletionTimestamp"] = "2026-11-16T00:00:00Z".into();
        // Only the production observer may start this drain. Removing its drain
        // call must fail at this receiver-entry deadline.
        tokio::time::timeout(Duration::from_secs(4), receiver.entered.notified()).await
            .expect("production readiness worker must initiate receiver drain");
        assert!(!manager.coordinator.can_acquire());
        assert!(!*manager.coordinator.failure_signal().borrow(), "planned deletion must keep local HTTP until SIGTERM");
        assert!(!authority.is_current());
        assert!(authority.run(async {}).await.is_err());
        assert_eq!(manager.coordinator.store.owner(&authority.token.agent_id).await.unwrap().unwrap(), original);
        assert!(!server.0.is_finished(), "HTTP exited before receiver ACK");
        assert_eq!(client.get(format!("{address}/ready")).send().await.unwrap().status(), StatusCode::SERVICE_UNAVAILABLE);
        receiver.acknowledgement.notify_one();
        tokio::time::timeout(Duration::from_secs(23), &mut observer.0).await.unwrap().unwrap();
        if refuse {
            assert!(*manager.coordinator.failure_signal().borrow());
            assert_eq!(manager.coordinator.store.owner(&authority.token.agent_id).await.unwrap().unwrap(), original);
        } else {
            assert!(!*manager.coordinator.failure_signal().borrow());
            assert!(manager.coordinator.store.owner(&authority.token.agent_id).await.unwrap().is_none());
            assert!(!server.0.is_finished(), "successful planned drain closed HTTP before SIGTERM");
            assert_eq!(client.get(format!("{address}/ready")).send().await.unwrap().status(), StatusCode::SERVICE_UNAVAILABLE);
            manager.drain().await.unwrap(); // preStop/SIGTERM retries remain idempotent.
        }
        stop.send(()).unwrap();
        let mut server = server;
        tokio::time::timeout(Duration::from_secs(3), &mut server.0).await.unwrap().unwrap().unwrap();
        assert!(!authority.is_current());
    }).catch_unwind().await;
    mongo.database(&database).drop().await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn planned_deletion_retains_http_until_signal_after_actual_receiver_ack() {
    mongo_planned_deletion(false).await;
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn planned_deletion_receiver_failure_remains_fatal_and_retains_durable_custody() {
    mongo_planned_deletion(true).await;
}
