use super::*;
use layer2_core::agent_ownership::{
    candidates, test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, OwnershipStore,
    PROTOCOL_VERSION,
};
use layer2_core::AgentId;
use std::sync::atomic::{AtomicBool, Ordering};

struct Probe(AtomicBool);
#[async_trait::async_trait]
impl ReadinessProbe for Probe {
    async fn ping(&self) -> Result<(), StoreError> {
        if self.0.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(StoreError::Unavailable(
                "synthetic unavailable dependency".into(),
            ))
        }
    }
}

#[tokio::test(start_paused = true)]
async fn serving_health_and_ownership_use_one_readiness_contract_without_a_startup_cycle() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::prepare(
        store.clone(),
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
    let probe = Arc::new(Probe(AtomicBool::new(false)));
    let workers = crate::worker_supervision::Workers::new();
    let (app, health) = ownership_fixture(probe.clone(), &coordinator, Some(workers.clone()));
    let serving = Arc::new(AtomicBool::new(false));
    coordinator
        .set_admission_readiness({
            let health = health.clone();
            let serving = serving.clone();
            Arc::new(move || serving.load(Ordering::Acquire) && health.ready())
        })
        .unwrap();
    let id = AgentId("startup-agent".into());
    tokio::task::yield_now().await;
    assert!(!health.ready());
    assert!(coordinator.activate().await.is_err());
    assert_eq!(
        tests::response(&app, "/ready").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    probe.0.store(true, Ordering::Release);
    tokio::time::advance(CHECK_INTERVAL).await;
    tokio::task::yield_now().await;
    assert!(
        !health.ready(),
        "unstarted required workers must still refuse"
    );
    for worker in workers.snapshot() {
        if worker.required {
            let activity = workers.get(worker.worker);
            activity.scheduled();
            activity.waiting(Duration::from_secs(1), "execution_slots");
        }
    }
    assert!(health.ready());
    assert!(
        coordinator.activate().await.is_err(),
        "listener is not yet serving"
    );
    assert!(candidates(&id, &store.membership().await.unwrap(), None)
        .unwrap()
        .is_empty());
    serving.store(true, Ordering::Release);
    coordinator.activate().await.unwrap();
    assert_eq!(tests::response(&app, "/ready").await.0, StatusCode::OK);
    let authority = coordinator.acquire(&id).await.unwrap().unwrap();
    probe.0.store(false, Ordering::Release);
    tokio::time::advance(CHECK_INTERVAL).await;
    tokio::task::yield_now().await;
    assert_eq!(
        tests::response(&app, "/ready").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(coordinator.acquire(&id).await.is_err());
    assert!(
        authority.is_current(),
        "dependency loss is not a receiver ACK"
    );
    coordinator.begin_drain().await;
    assert!(!authority.is_current());
    assert_eq!(
        tests::response(&app, "/ready").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(tests::response(&app, "/livez").await.0, StatusCode::OK);
}
