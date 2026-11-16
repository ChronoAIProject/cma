use super::*;
use axum::{
    body::Body,
    http::{Request, Response, StatusCode},
};
use layer2_core::agent_ownership::{test_support::MemoryOwnershipStore, OwnershipStore};
use std::sync::atomic::AtomicUsize;
use tower::ServiceExt;

struct StoreProbe;
#[async_trait::async_trait]
impl crate::health::ReadinessProbe for StoreProbe {
    async fn ping(&self) -> Result<(), layer2_core::StoreError> {
        Ok(())
    }
}

fn pod(running: bool) -> serde_json::Value {
    serde_json::json!({"apiVersion":"v1","kind":"Pod",
        "metadata":{"name":"manager","namespace":"test","uid":"original"},
        "status":{"phase":if running {"Running"} else {"Pending"},
            "conditions":[{"type":"Ready","status":"False"}],
            "containerStatuses":[{"name":"api", "containerID":"containerd://original",
                "restartCount":0,"started":running,
                "state":if running {serde_json::json!({"running":{"startedAt":"2026-11-16T00:00:00Z"}})} else {serde_json::json!({"waiting":{}})} }]}})
}

fn identity() -> ManagerPod {
    ManagerPod {
        name: "manager".into(),
        namespace: "test".into(),
        uid: "original".into(),
    }
}

#[test]
fn exact_running_container_proof_is_independent_of_pod_ready() {
    let mut raw = pod(false);
    let parse =
        |raw: &serde_json::Value| serde_json::from_value::<DynamicObject>(raw.clone()).unwrap();
    assert_eq!(
        pod_state(&parse(&raw), &identity()).unwrap(),
        PodState::Pending
    );
    raw["status"]["conditions"][0]["status"] = "True".into();
    assert_eq!(
        pod_state(&parse(&raw), &identity()).unwrap(),
        PodState::Pending
    );
    raw = pod(true); // Kubernetes Ready remains false: no readiness cycle.
    assert!(matches!(
        pod_state(&parse(&raw), &identity()).unwrap(),
        PodState::Running(_)
    ));
    raw["status"]["containerStatuses"][0]["name"] = "sidecar".into();
    assert_eq!(
        pod_state(&parse(&raw), &identity()).unwrap(),
        PodState::Pending
    );
    raw["status"]["containerStatuses"][0]["name"] = "api".into();
    raw["metadata"]["deletionTimestamp"] = "2026-11-16T00:00:00Z".into();
    assert_eq!(
        pod_state(&parse(&raw), &identity()).unwrap(),
        PodState::Terminating
    );
    raw["metadata"]["uid"] = "replacement".into();
    assert!(pod_state(&parse(&raw), &identity()).is_err());
}

async fn ready(router: &axum::Router) -> StatusCode {
    router
        .clone()
        .oneshot(Request::get("/ready").body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

fn reader(reply: Arc<Mutex<serde_json::Value>>, mode: Arc<AtomicUsize>) -> PodReader {
    let service = tower::service_fn({
        let reply = reply.clone();
        let mode = mode.clone();
        move |request: Request<kube::client::Body>| {
            let reply = reply.clone();
            let mode = mode.clone();
            async move {
                assert_eq!(request.method(), "GET");
                assert_eq!(request.uri().path(), "/api/v1/namespaces/test/pods/manager");
                match mode.load(Ordering::SeqCst) {
                    1 => Err(std::io::Error::other("fixture read unavailable")),
                    2 => std::future::pending().await,
                    _ => Ok(Response::new(Body::from(reply.lock().unwrap().to_string()))),
                }
            }
        }
    });
    let resource = ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Pod"));
    PodReader {
        api: Api::namespaced_with(Client::new(service, "test"), "test", &resource),
        identity: identity(),
        container: Mutex::new(None),
    }
}

// Exercise the production Kubernetes reader, admission monitor, cached health
// route and actual coordinator. No Mongo server or Kubernetes cluster is used.
#[tokio::test(start_paused = true)]
async fn native_proof_readiness_and_ownership_admission_stay_coherent() {
    let reply = Arc::new(Mutex::new(pod(false)));
    // 0 returns Pod JSON, 1 is a read error, 2 is an unanswered read.
    let mode = Arc::new(AtomicUsize::new(0));
    let reader = reader(reply.clone(), mode.clone());
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::prepare(
        store.client(),
        Member {
            instance_id: "manager".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://manager:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        },
    );
    let (router, health) =
        crate::health::ownership_fixture(Arc::new(StoreProbe), &coordinator, None);
    tokio::task::yield_now().await;
    assert!(health.ready());
    let (stop, signal) = tokio::sync::oneshot::channel();
    let termination = Termination::observe(async {
        let _ = signal.await;
    });
    let probe =
        AdmissionProbe::install(&coordinator, Some(reader), health, termination.clone()).unwrap();
    let id = AgentId("existing".into());
    for _ in 0..6 {
        assert!(probe.check(&coordinator).await);
        assert_eq!(ready(&router).await, StatusCode::SERVICE_UNAVAILABLE);
        assert!(coordinator.acquire(&id).await.is_err());
        assert!(store.membership().await.unwrap().members.is_empty());
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
    }
    *reply.lock().unwrap() = pod(true);
    assert!(probe.check(&coordinator).await);
    assert_eq!(ready(&router).await, StatusCode::OK);
    let authority = coordinator.acquire(&id).await.unwrap().unwrap();
    mode.store(1, Ordering::SeqCst);
    assert!(probe.check(&coordinator).await);
    assert_eq!(ready(&router).await, StatusCode::SERVICE_UNAVAILABLE);
    assert!(coordinator.acquire(&id).await.is_err());
    coordinator.maintain().await.unwrap();
    assert!(
        authority.is_current(),
        "proof withdrawal must preserve safe retained renewal"
    );
    mode.store(0, Ordering::SeqCst);
    assert!(probe.check(&coordinator).await);
    assert_eq!(ready(&router).await, StatusCode::OK);
    tokio::time::advance(POD_PROOF_MAX_AGE).await;
    tokio::task::yield_now().await;
    assert_eq!(ready(&router).await, StatusCode::SERVICE_UNAVAILABLE);
    assert!(coordinator.acquire(&id).await.is_err());
    mode.store(2, Ordering::SeqCst);
    assert!(probe.check(&coordinator).await); // deadline is enforced by the real reader
    assert_eq!(ready(&router).await, StatusCode::SERVICE_UNAVAILABLE);
    mode.store(0, Ordering::SeqCst);
    assert!(probe.check(&coordinator).await);
    assert_eq!(ready(&router).await, StatusCode::OK);
    reply.lock().unwrap()["metadata"]["deletionTimestamp"] = "2026-11-16T00:00:00Z".into();
    assert!(!probe.check(&coordinator).await);
    assert_eq!(ready(&router).await, StatusCode::SERVICE_UNAVAILABLE);
    let held = coordinator.begin_drain().await;
    assert!(!authority.is_current());
    assert_eq!(held.len(), 1);
    coordinator
        .handoff(&held[0], async { Ok(()) })
        .await
        .unwrap();
    assert!(store.owner(&id).await.unwrap().is_none());
    *reply.lock().unwrap() = pod(true);
    assert!(probe.check(&coordinator).await);
    assert!(coordinator.activate().await.is_err());
    assert_eq!(ready(&router).await, StatusCode::SERVICE_UNAVAILABLE);
    stop.send(()).unwrap();
    termination.cancelled().await;
    assert!(!probe.check(&coordinator).await);
}

#[tokio::test(start_paused = true)]
async fn never_ready_scale_up_observes_deletion_or_signal_without_owning_agents() {
    use layer2_core::agent_ownership::{candidates, HEARTBEAT_INTERVAL_MS};
    for deletion in [false, true] {
        let store = Arc::new(MemoryOwnershipStore::default());
        let member = |name: &str| Member {
            instance_id: name.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: format!("http://{name}:8080"),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        };
        let first = OwnershipCoordinator::register(store.client(), member("serving-a"))
            .await
            .unwrap();
        let second = OwnershipCoordinator::register(store.client(), member("serving-b"))
            .await
            .unwrap();
        let peers = [&first, &second];
        let starting = OwnershipCoordinator::prepare(store.client(), member("scale-up"));
        let workers = crate::worker_supervision::Workers::new();
        let (router, health) = crate::health::ownership_fixture(
            Arc::new(StoreProbe),
            &starting,
            Some(workers.clone()),
        );
        tokio::task::yield_now().await;
        assert!(!health.ready(), "workers have not completed startup");
        let reply = Arc::new(Mutex::new(pod(true)));
        let (stop, signal) = tokio::sync::oneshot::channel();
        let termination = Termination::observe(async {
            let _ = signal.await;
        });
        let probe = AdmissionProbe::install(
            &starting,
            Some(reader(reply.clone(), Arc::new(AtomicUsize::new(0)))),
            health,
            termination.clone(),
        )
        .unwrap();
        let ids: Vec<_> = (0..31).map(|n| AgentId(format!("scale-up-{n}"))).collect();
        let mut retained = Vec::new();
        for id in &ids {
            let membership = store.membership().await.unwrap();
            let selected = &candidates(id, &membership, None).unwrap()[0].instance_id;
            let peer = peers
                .iter()
                .find(|peer| &peer.instance_id == selected)
                .unwrap();
            retained.push(peer.acquire(id).await.unwrap().unwrap());
        }
        // Model the failed third AM's prolonged startup while the two serving
        // AMs keep real fenced ownership current beyond a lease lifetime.
        for _ in 0..6 {
            tokio::time::advance(Duration::from_millis(HEARTBEAT_INTERVAL_MS)).await;
            tokio::task::yield_now().await;
            for peer in peers {
                peer.maintain().await.unwrap();
            }
            assert!(probe.check(&starting).await);
            assert_eq!(ready(&router).await, StatusCode::SERVICE_UNAVAILABLE);
            assert!(starting.is_starting());
            for id in &ids {
                assert!(starting.acquire(id).await.is_err());
            }
            assert!(starting.owned().await.is_empty());
            assert_eq!(store.membership().await.unwrap().members.len(), 2);
        }
        let terminated_at = Instant::now();
        if deletion {
            reply.lock().unwrap()["metadata"]["deletionTimestamp"] = "2026-11-16T00:00:00Z".into();
        } else {
            stop.send(()).unwrap();
            termination.cancelled().await;
        }
        assert!(!probe.check(&starting).await);
        assert!(!starting.can_acquire());
        assert_eq!(ready(&router).await, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            tokio::time::timeout(Duration::from_secs(23), starting.begin_drain())
                .await
                .unwrap()
                .is_empty()
        );
        for authority in &retained {
            assert!(authority.is_current());
            assert_eq!(
                store
                    .owner(&authority.token.agent_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .token,
                authority.token
            );
            assert!(peers
                .iter()
                .any(|peer| peer.instance_id == authority.token.instance_id && peer.can_acquire()));
        }
        assert!(terminated_at.elapsed() < Duration::from_secs(23));
        // Completing workers and receiving a late healthy Pod response must
        // not resurrect the terminated process during its long Pod grace.
        for worker in workers.snapshot() {
            let activity = workers.get(worker.worker);
            activity.scheduled();
            activity.waiting(Duration::from_secs(30), "fixture ready");
        }
        *reply.lock().unwrap() = pod(true);
        let _ = probe.check(&starting).await;
        assert!(starting.activate().await.is_err());
        assert_eq!(ready(&router).await, StatusCode::SERVICE_UNAVAILABLE);
        assert!(starting.owned().await.is_empty());
    }
}

#[tokio::test]
async fn changed_container_incarnation_is_rejected() {
    for changed_id in [false, true] {
        let reply = Arc::new(Mutex::new(pod(true)));
        let reader = reader(reply.clone(), Arc::new(AtomicUsize::new(0)));
        assert!(matches!(
            reader.state().await.unwrap(),
            PodState::Running(_)
        ));
        if changed_id {
            reply.lock().unwrap()["status"]["containerStatuses"][0]["containerID"] =
                "containerd://replacement".into();
        } else {
            reply.lock().unwrap()["status"]["containerStatuses"][0]["restartCount"] = 1.into();
        }
        assert_eq!(reader.state().await.unwrap(), PodState::Replaced);
    }
}

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
        assert!(probe.check(&manager.coordinator).await);
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
        assert!(!probe.check(&manager.coordinator).await);
        assert!(!manager.coordinator.can_acquire());
        assert!(!*manager.coordinator.failure_signal().borrow(), "planned deletion must keep local HTTP until SIGTERM");
        let entered = receiver.entered.notified();
        let drain = manager.drain();
        tokio::pin!(drain);
        tokio::select! {
            _ = entered => (),
            result = &mut drain => panic!("drain ended before receiver acknowledgement: {result:?}"),
        }
        assert!(!authority.is_current());
        assert!(authority.run(async {}).await.is_err());
        assert_eq!(manager.coordinator.store.owner(&authority.token.agent_id).await.unwrap().unwrap(), original);
        assert!(!server.0.is_finished(), "HTTP exited before receiver ACK");
        assert_eq!(client.get(format!("{address}/ready")).send().await.unwrap().status(), StatusCode::SERVICE_UNAVAILABLE);
        receiver.acknowledgement.notify_one();
        let outcome = tokio::time::timeout(Duration::from_secs(23), &mut drain).await.unwrap();
        if refuse {
            assert!(outcome.is_err());
            assert!(*manager.coordinator.failure_signal().borrow());
            assert_eq!(manager.coordinator.store.owner(&authority.token.agent_id).await.unwrap().unwrap(), original);
        } else {
            outcome.unwrap();
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
