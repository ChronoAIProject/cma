use super::super::*;
use crate::{agent_manager::AgentManager, sandbox::LiveAgentRegistry};
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use std::sync::atomic::{AtomicBool, Ordering};

struct Receiver(Arc<AtomicBool>);

#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "drain-test-runtime".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        self.0.store(true, Ordering::Release);
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
            manager.activate().unwrap();
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
        authority
            .register_receiver(Arc::new(Receiver(revoked.clone())))
            .unwrap();
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
        managers[owner].drain().await;
        for manager in &managers {
            manager.drain().await;
        }
        drop(instances);
        drop(managers);
        mongo.database(&name).drop().await.unwrap();
    }
}
