use super::super::*;
use crate::{agent_manager::AgentManager, sandbox::LiveAgentRegistry};
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tower::ServiceExt;

struct Receiver {
    revoked: Arc<AtomicBool>,
    lifecycle_exited: Arc<AtomicBool>,
    attempts: AtomicUsize,
    acknowledgement_delay: Option<Duration>,
}

struct LifecycleExit(Arc<AtomicBool>);
impl Drop for LifecycleExit {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "drain-test-runtime".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        assert!(
            self.lifecycle_exited.load(Ordering::Acquire),
            "receiver revoked before lifecycle join"
        );
        let attempt = self.attempts.fetch_add(1, Ordering::Relaxed);
        if let Some(delay) = self.acknowledgement_delay {
            if attempt == 0 {
                return Err(layer3_adapter::AdapterError::Unavailable(
                    "synthetic first revoke transport failure".into(),
                ));
            }
            tokio::time::sleep(delay).await;
        }
        self.revoked.store(true, Ordering::Release);
        Ok(())
    }
}

fn key() -> ring::hmac::Key {
    ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-drain-key")
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn authenticated_prestop_drains_actual_managers_before_peer_takeover() {
    use crate::store::sandbox_maintenance::mongo_integration_tests::database;
    for count in [2, 3] {
        let (mongo, name, durable) = database().await;
        let mut managers = Vec::new();
        let mut instances = Vec::new();
        let mut ports = Vec::new();
        let mut lifecycle_exits = Vec::new();
        for index in 0..count {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            ports.push(listener.local_addr().unwrap().port());
            let address = format!("http://{}", listener.local_addr().unwrap());
            let durable = Arc::new(
                durable
                    .clone()
                    .enable_agent_ownership(format!("manager-{index}"))
                    .await
                    .unwrap(),
            );
            let manager = AgentManager::register(
                durable,
                Arc::new(LiveAgentRegistry::default()),
                address.clone(),
                256,
                false,
                false,
            )
            .await
            .unwrap();
            let updates =
                DurableAgentUpdateRecorder::new(Arc::new(InMemorySandboxStore::default()));
            let relay = Relay::new(manager.coordinator.clone(), &updates, key()).unwrap();
            let router = relay.router(
                Router::new().route("/ready", get(|| async { StatusCode::OK })),
                manager.clone(),
            );
            let router = crate::with_request_context(router);
            let lifecycle = crate::agent_lifecycle::LifecycleDrain::fixture(1);
            let exited = Arc::new(AtomicBool::new(false));
            let exit = LifecycleExit(exited.clone());
            assert!(lifecycle.spawn_fixture(async move {
                let _exit = exit;
                std::future::pending::<()>().await;
            }));
            manager.register_lifecycle(lifecycle).unwrap();
            lifecycle_exits.push(exited);
            manager.activate().await.unwrap();
            let server = tokio::spawn(async move {
                axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .await
                .unwrap();
            });
            instances.push(Instance {
                coordinator: manager.coordinator.clone(),
                relay,
                updates,
                address,
                server,
            });
            managers.push(manager);
        }
        let id = AgentId("drain-agent".into());
        let membership = managers[0].coordinator.store.membership().await.unwrap();
        let preferred = candidates(&id, &membership, None).unwrap()[0]
            .instance_id
            .clone();
        let owner = managers
            .iter()
            .position(|manager| manager.coordinator.instance_id == preferred)
            .unwrap();
        let authority = managers[owner]
            .coordinator
            .acquire(&id)
            .await
            .unwrap()
            .unwrap();
        let revoked = Arc::new(AtomicBool::new(false));
        let receiver = Arc::new(Receiver {
            revoked: revoked.clone(),
            lifecycle_exited: lifecycle_exits[owner].clone(),
            attempts: AtomicUsize::new(0),
            // Exercise a valid reply beyond the independent store-operation limit.
            acknowledgement_delay: Some(Duration::from_millis(2_500)),
        });
        authority.register_receiver(receiver.clone()).unwrap();
        let waiting = tokio::spawn({
            let authority = authority.clone();
            async move { authority.run(std::future::pending::<()>()).await }
        });
        let url = &instances[owner].address;
        assert_eq!(
            client()
                .get(format!("{url}/ready"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            client()
                .post(format!("{url}{PREFIX}/drain"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert!(managers[owner].is_ready());
        control::drain_local(ports[owner], key()).await.unwrap();
        assert!(revoked.load(Ordering::Acquire));
        assert_eq!(receiver.attempts.load(Ordering::Relaxed), 2);
        assert!(waiting.await.unwrap().is_err());
        assert_eq!(
            client()
                .get(format!("{url}/ready"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(managers[owner]
            .coordinator
            .store
            .owner(&id)
            .await
            .unwrap()
            .is_none());
        let mut successor = None;
        for (index, manager) in managers.iter().enumerate() {
            if index != owner {
                if let Some(next) = manager.coordinator.acquire(&id).await.unwrap() {
                    assert!(successor.replace(next).is_none());
                }
            }
        }
        assert!(successor.unwrap().token.epoch > authority.token.epoch);
        // SIGTERM uses this same idempotent drain after preStop has finished.
        managers[owner].drain().await.unwrap();
        for manager in &managers {
            manager.drain().await.unwrap();
        }
        drop(instances);
        drop(managers);
        mongo.database(&name).drop().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn prestop_and_main_retain_ownership_until_blocked_lifecycle_has_joined() {
    use crate::store::sandbox_maintenance::mongo_integration_tests::database;
    let (mongo, name, durable) = database().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let address = format!("http://127.0.0.1:{port}");
    let manager = AgentManager::register(
        Arc::new(
            durable
                .enable_agent_ownership("blocked-manager".into())
                .await
                .unwrap(),
        ),
        Arc::new(LiveAgentRegistry::default()),
        address.clone(),
        256,
        false,
        false,
    )
    .await
    .unwrap();
    let lifecycle = crate::agent_lifecycle::LifecycleDrain::fixture(1);
    let exited = Arc::new(AtomicBool::new(false));
    let exit = LifecycleExit(exited.clone());
    let (unblock, blocked) = std::sync::mpsc::channel::<()>();
    let (entered, ready) = tokio::sync::oneshot::channel();
    assert!(lifecycle.spawn_fixture(async move {
        let _exit = exit;
        entered.send(()).unwrap();
        // A bounded synchronous fixture makes cancellation incomplete until
        // explicitly released. It cannot leave a hung test worker on failure.
        let _ = blocked.recv_timeout(Duration::from_secs(12));
    }));
    ready.await.unwrap();
    manager.register_lifecycle(lifecycle.clone()).unwrap();
    manager.activate().await.unwrap();
    let authority = manager
        .coordinator
        .acquire(&AgentId("blocked-agent".into()))
        .await
        .unwrap()
        .unwrap();
    let original = manager
        .coordinator
        .store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    let revoked = Arc::new(AtomicBool::new(false));
    authority
        .register_receiver(Arc::new(Receiver {
            revoked: revoked.clone(),
            lifecycle_exited: exited.clone(),
            attempts: AtomicUsize::new(0),
            acknowledgement_delay: None,
        }))
        .unwrap();
    let updates = DurableAgentUpdateRecorder::new(Arc::new(InMemorySandboxStore::default()));
    let relay = Relay::new(manager.coordinator.clone(), &updates, key()).unwrap();
    let router = relay.router(Router::new(), manager.clone());
    let router = crate::with_request_context(router);
    let mut remote = axum::http::Request::post(control::DRAIN)
        .extension(axum::extract::ConnectInfo(
            "192.0.2.7:1234".parse::<std::net::SocketAddr>().unwrap(),
        ))
        .body(axum::body::Body::empty())
        .unwrap();
    *remote.headers_mut() = control::signed_drain_headers(key()).unwrap();
    assert_eq!(
        router.clone().oneshot(remote).await.unwrap().status(),
        StatusCode::FORBIDDEN,
    );
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let instance = Instance {
        coordinator: manager.coordinator.clone(),
        relay,
        updates,
        address,
        server,
    };
    let wrong_key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"other-drain-fixture");
    assert_eq!(
        control::drain_local(port, wrong_key).await.unwrap_err(),
        "local Agent Manager rejected drain (HTTP 403)",
    );
    // Each refusal exercises the real control route with request context and
    // leaves the current owner and blocked lifecycle intact.
    for headers in [
        {
            let mut headers = control::signed_drain_headers(key()).unwrap();
            headers.insert("x-request-id", "different-fixture-request".parse().unwrap());
            headers
        },
        {
            let mut headers = control::signed_drain_headers(key()).unwrap();
            headers.append(PROOF, headers[PROOF].clone());
            headers
        },
    ] {
        assert_eq!(
            client()
                .post(format!("{}{}", instance.address, control::DRAIN))
                .headers(headers)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN,
        );
    }
    assert!(manager.is_ready());
    assert!(authority.is_current());
    assert!(!revoked.load(Ordering::Acquire));
    assert!(!exited.load(Ordering::Acquire));
    let result = control::drain_local(port, key()).await;
    assert!(
        result.is_err(),
        "incomplete drain must not return HTTP success"
    );
    assert!(!manager.is_ready());
    assert!(!authority.is_current());
    assert!(!revoked.load(Ordering::Acquire));
    assert!(!exited.load(Ordering::Acquire));
    assert_eq!(
        manager
            .coordinator
            .store
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .unwrap(),
        original
    );
    unblock.send(()).unwrap();
    let mut shutdown = crate::shutdown::ShutdownCoordinator::new();
    shutdown.register_lifecycle(lifecycle);
    shutdown.drain_agent_manager(Some(&manager)).await.unwrap();
    assert!(exited.load(Ordering::Acquire));
    assert!(revoked.load(Ordering::Acquire));
    assert!(manager
        .coordinator
        .store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_none());
    control::drain_local(port, key()).await.unwrap();
    drop(instance);
    drop(manager);
    assert_eq!(shutdown.drain().await, 0);
    mongo.database(&name).drop().await.unwrap();
}
