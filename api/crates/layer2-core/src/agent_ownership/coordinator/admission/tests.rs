use super::*;
use crate::agent_ownership::{test_support::MemoryOwnershipStore, PROTOCOL_VERSION};

#[tokio::test(start_paused = true)]
async fn dormant_manager_never_advertises_placement_before_serving_readiness() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::prepare(store.clone(), member());
    let ready = Arc::new(AtomicBool::new(false));
    coordinator
        .set_admission_readiness({
            let ready = ready.clone();
            Arc::new(move || ready.load(Ordering::Acquire))
        })
        .unwrap();
    let id = AgentId("dormant-agent".into());
    for _ in 0..10 {
        tokio::time::advance(Duration::from_secs(3)).await;
        assert!(coordinator.maintain().await.unwrap().is_empty());
        assert!(coordinator.activate().await.is_err());
        assert!(coordinator.acquire(&id).await.is_err());
        let membership = store.membership().await.unwrap();
        assert!(candidates(&id, &membership, None).unwrap().is_empty());
        assert!(!store.has_owner_history(&id).await.unwrap());
    }
    ready.store(true, Ordering::Release);
    coordinator.activate().await.unwrap();
    let authority = coordinator.acquire(&id).await.unwrap().unwrap();
    ready.store(false, Ordering::Release);
    assert!(!coordinator.can_acquire());
    assert!(coordinator.acquire(&id).await.is_err());
    coordinator.maintain().await.unwrap();
    assert!(
        authority.is_current(),
        "readiness loss must not fake a receiver revoke"
    );
    let owned = coordinator.begin_drain().await;
    assert_eq!(owned.len(), 1);
    assert!(!authority.is_current());
    ready.store(true, Ordering::Release);
    assert!(coordinator.activate().await.is_err());
}

#[tokio::test(start_paused = true)]
async fn startup_drain_prevents_later_registration_without_store_writes() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::prepare(store.clone(), member());
    assert!(coordinator.begin_drain().await.is_empty());
    assert!(coordinator.activate().await.is_err());
    assert!(store.membership().await.unwrap().members.is_empty());
    assert!(!coordinator.uncertain_registration());
    assert_eq!(coordinator.uncertain_acquisitions(), 0);
}

fn member() -> Member {
    Member {
        instance_id: "manager".into(),
        protocol: PROTOCOL_VERSION,
        relay_address: "http://manager:8080".into(),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: 256,
    }
}

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
