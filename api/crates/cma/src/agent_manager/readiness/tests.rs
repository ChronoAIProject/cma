use super::*;
use crate::health::ReadinessProbe;
use layer2_core::{agent_ownership::OwnershipStore, StoreError};

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn actual_manager_drain_reports_unconfirmed_acquisition_after_settling_known_tokens() {
    use futures_util::TryStreamExt;
    use mongodb::bson::doc;

    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    let store = Arc::new(
        durable
            .enable_agent_ownership("uncertain-manager".into())
            .await
            .unwrap(),
    );
    let manager = AgentManager::register(
        store.clone(),
        Arc::new(LiveAgentRegistry::default()),
        "http://manager:8080".into(),
        256,
    )
    .await
    .unwrap();
    manager
        .register_lifecycle(crate::agent_lifecycle::LifecycleDrain::fixture(1))
        .unwrap();
    manager.activate().unwrap();
    let known = manager
        .coordinator
        .acquire(&AgentId("known".into()))
        .await
        .unwrap()
        .unwrap();
    let database = mongo.database(&name);
    let original = database
        .list_collections()
        .filter(doc! {"name":"cma_agent_manager_owners"})
        .await
        .unwrap()
        .try_next()
        .await
        .unwrap()
        .unwrap()
        .options
        .validator
        .unwrap_or_default();
    // Preserve the real schema while rejecting only this synthetic write.
    // The coordinator conservatively retains an unsuccessful write as unknown;
    // held committed replies are covered by the ownership-store regressions.
    database
        .run_command(doc! {
            "collMod":"cma_agent_manager_owners",
            "validator":{"$and":[original, {"_id":{"$ne":"rejected"}}]},
        })
        .await
        .unwrap();
    assert!(manager
        .coordinator
        .acquire(&AgentId("rejected".into()))
        .await
        .is_err());
    assert_eq!(manager.coordinator.uncertain_acquisitions(), 1);
    assert_eq!(
        manager.drain().await,
        Err("Agent Manager drain has unconfirmed ownership writes".into())
    );
    assert!(!manager.is_ready());
    assert!(!known.is_current());
    assert!(manager.coordinator.owned().await.is_empty());
    assert!(store
        .agent_ownership
        .as_ref()
        .unwrap()
        .owner(&known.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(manager.coordinator.uncertain_acquisitions(), 1);
    assert!(
        manager.drain().await.is_err(),
        "a repeat cannot conceal retained uncertainty"
    );
    database.drop().await.unwrap();
}

struct Probe(AtomicBool);

#[async_trait::async_trait]
impl ReadinessProbe for Probe {
    async fn ping(&self) -> Result<(), StoreError> {
        if self.0.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(StoreError::Unavailable(
                "synthetic startup dependency".into(),
            ))
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn actual_manager_waits_for_serving_and_dependencies_then_drains_without_reactivation() {
    use crate::store::sandbox_maintenance::mongo_integration_tests::database;
    for become_ready in [false, true] {
        let (mongo, name, durable) = database().await;
        let store = Arc::new(
            durable
                .enable_agent_ownership("readiness-manager".into())
                .await
                .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let manager = AgentManager::prepare(
            store.clone(),
            Arc::new(LiveAgentRegistry::default()),
            address.clone(),
            256,
        )
        .await
        .unwrap();
        manager
            .register_lifecycle(crate::agent_lifecycle::LifecycleDrain::fixture(1))
            .unwrap();
        let probe = Arc::new(Probe(AtomicBool::new(false)));
        let (router, health) =
            crate::health::ownership_fixture(probe.clone(), &manager.coordinator, None);
        let (terminate, signal) = tokio::sync::oneshot::channel();
        let termination = Termination::observe(async {
            let _ = signal.await;
        });
        let observing = manager.spawn_readiness(health, termination).unwrap();
        let (stop_http, http_signal) = tokio::sync::oneshot::channel();
        let http = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = http_signal.await;
                })
                .await
                .unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        assert_eq!(
            client
                .get(format!("{address}/ready"))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(!manager.is_ready());
        assert!(manager
            .coordinator
            .store
            .membership()
            .await
            .unwrap()
            .members
            .is_empty());
        manager.activate().unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(manager
            .coordinator
            .store
            .membership()
            .await
            .unwrap()
            .members
            .is_empty());
        let mut authority = None;
        if become_ready {
            probe.0.store(true, Ordering::Release);
            tokio::time::timeout(Duration::from_secs(5), async {
                while !manager.is_ready() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                client
                    .get(format!("{address}/ready"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                reqwest::StatusCode::OK
            );
            authority = manager
                .coordinator
                .acquire(&AgentId("ready-agent".into()))
                .await
                .unwrap();
            assert!(authority.as_ref().unwrap().is_current());
        }
        terminate.send(()).unwrap();
        observing.await.unwrap();
        assert!(!manager.is_ready());
        manager.drain().await.unwrap();
        assert!(manager.coordinator.owned().await.is_empty());
        if let Some(authority) = authority {
            assert!(!authority.is_current());
            assert!(store
                .agent_ownership
                .as_ref()
                .unwrap()
                .owner(&authority.token.agent_id)
                .await
                .unwrap()
                .is_none());
        } else {
            assert!(manager
                .coordinator
                .store
                .membership()
                .await
                .unwrap()
                .members
                .is_empty());
        }
        assert!(manager.coordinator.activate().await.is_err());
        assert_eq!(
            client
                .get(format!("{address}/ready"))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            client
                .get(format!("{address}/livez"))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::OK
        );
        stop_http.send(()).unwrap();
        http.await.unwrap();
        mongo.database(&name).drop().await.unwrap();
    }
}
