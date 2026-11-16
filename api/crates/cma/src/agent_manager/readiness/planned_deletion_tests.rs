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

use layer2_core::agent_ownership::{Membership, OwnershipError, OwnershipLease, OwnershipToken};

struct MongoMaintenanceGate {
    inner: Arc<dyn OwnershipStore>,
    operation: usize,
    durable_drain_entries: AtomicUsize,
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl OwnershipStore for MongoMaintenanceGate {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
    }
    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        if draining {
            self.durable_drain_entries.fetch_add(1, Ordering::SeqCst);
        }
        if !draining && self.operation == 1 {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        self.inner.heartbeat(id, draining).await
    }
    async fn membership(&self) -> Result<Membership, OwnershipError> {
        self.inner.membership().await
    }
    async fn owner(&self, id: &AgentId) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.inner.owner(id).await
    }
    async fn has_owner_history(&self, id: &AgentId) -> Result<bool, OwnershipError> {
        self.inner.has_owner_history(id).await
    }
    async fn acquire(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.inner.acquire(id, instance).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        if self.operation == 2 {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        // Cross the real Mongo shared-member fence only after the barrier.
        self.inner.renew(token).await
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

async fn mongo_maintenance_before_drain(operation: usize) {
    use futures_util::FutureExt;
    use std::panic::AssertUnwindSafe;
    let (mongo, database, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    let result = AssertUnwindSafe(async {
        let durable = durable
            .enable_agent_ownership("mongo-ordered-manager".into())
            .await
            .unwrap();
        let store = Arc::new(MongoMaintenanceGate {
            inner: durable.agent_ownership.unwrap(),
            operation,
            durable_drain_entries: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
        });
        let coordinator = OwnershipCoordinator::register(
            store.clone(),
            Member {
                instance_id: "mongo-ordered-manager".into(),
                protocol: PROTOCOL_VERSION,
                relay_address: "http://127.0.0.1:8080".into(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 4,
            },
        )
        .await
        .unwrap();
        let authority = coordinator
            .acquire(&AgentId("mongo-maintenance-agent".into()))
            .await
            .unwrap()
            .unwrap();
        let running = tokio::spawn({
            let coordinator = coordinator.clone();
            async move { coordinator.maintain().await }
        });
        tokio::time::timeout(Duration::from_secs(3), store.entered.notified())
            .await
            .unwrap();
        let drain = coordinator.begin_drain();
        tokio::pin!(drain);
        let first_poll = futures_util::poll!(&mut drain);
        let durable_drain_before_resume = store.durable_drain_entries.load(Ordering::SeqCst);
        assert!(!coordinator.can_acquire());
        assert!(!authority.is_current());
        assert!(authority.run(async {}).await.is_err());
        store.resume.notify_one();
        let (maintained, held) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(running, async {
                match first_poll {
                    std::task::Poll::Ready(held) => held,
                    std::task::Poll::Pending => drain.await,
                }
            })
        })
        .await
        .unwrap();
        assert_eq!(
            durable_drain_before_resume, 0,
            "durable drain must join pre-store Mongo maintenance"
        );
        assert_eq!(store.durable_drain_entries.load(Ordering::SeqCst), 1);
        assert!(
            maintained.unwrap().is_ok(),
            "planned drain must preserve successful Mongo member fence"
        );
        assert!(!*coordinator.failure_signal().borrow());
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].token, authority.token);
        assert!(store
            .membership()
            .await
            .unwrap()
            .members
            .iter()
            .all(|member| member.draining));
        let retained = store
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .unwrap();
        let entered = tokio::sync::Notify::new();
        let ack = tokio::sync::Notify::new();
        let handoff = coordinator.handoff(&authority, async {
            entered.notify_one();
            ack.notified().await;
            Ok(())
        });
        tokio::pin!(handoff);
        tokio::select! {
            _ = entered.notified() => (),
            result = &mut handoff => panic!("durable release preceded receiver ACK: {result:?}"),
        }
        assert_eq!(
            store
                .owner(&authority.token.agent_id)
                .await
                .unwrap()
                .unwrap(),
            retained
        );
        ack.notify_one();
        handoff.await.unwrap();
        assert!(store
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .is_none());
        assert!(!authority.is_current());
        assert!(!*coordinator.failure_signal().borrow());
    })
    .catch_unwind()
    .await;
    mongo.database(&database).drop().await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn real_mongo_heartbeat_is_joined_before_durable_planned_drain() {
    mongo_maintenance_before_drain(1).await;
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn real_mongo_owner_renewal_is_joined_before_durable_planned_drain() {
    mongo_maintenance_before_drain(2).await;
}
