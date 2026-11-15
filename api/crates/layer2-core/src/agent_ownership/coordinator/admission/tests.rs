use super::*;
use crate::agent_ownership::{test_support::MemoryOwnershipStore, PROTOCOL_VERSION};

#[tokio::test(start_paused = true)]
async fn cached_authority_rechecks_admission_after_owned_map_contention() {
    for terminating in [false, true] {
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
        let ready = Arc::new(AtomicBool::new(true));
        coordinator
            .set_admission_readiness({
                let ready = ready.clone();
                Arc::new(move || ready.load(Ordering::Acquire))
            })
            .unwrap();
        coordinator.activate().await.unwrap();
        let id = AgentId("retained".into());
        let authority = coordinator.acquire(&id).await.unwrap().unwrap();
        let held = coordinator.owned.lock().await;
        let acquiring = coordinator.acquire(&id);
        tokio::pin!(acquiring);
        assert!(futures_util::poll!(&mut acquiring).is_pending());
        if terminating {
            coordinator.stop_admission();
        } else {
            ready.store(false, Ordering::Release);
        }
        drop(held);
        assert!(
            acquiring.await.is_err(),
            "cached authority escaped closed admission"
        );
        if !terminating {
            coordinator.maintain().await.unwrap();
            assert!(authority.is_current());
            ready.store(true, Ordering::Release);
            assert!(Arc::ptr_eq(
                &authority,
                &coordinator.acquire(&id).await.unwrap().unwrap()
            ));
        }
        let owned = coordinator.begin_drain().await;
        assert_eq!(owned.len(), 1);
        assert!(!authority.is_current());
    }
}
