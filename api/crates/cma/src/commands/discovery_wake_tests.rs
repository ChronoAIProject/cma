use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipStore, PROTOCOL_VERSION,
};

async fn register(store: Arc<MemoryOwnershipStore>, name: &str) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store,
        Member {
            instance_id: name.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: format!("http://{name}:8080"),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        },
    )
    .await
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn two_am_notification_requires_current_local_owner_without_acquiring() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let a = register(store.clone(), "a").await;
    let b = register(store.clone(), "b").await;
    let agent = AgentId("agent".into());
    let signals = [Arc::new(Notify::new()), Arc::new(Notify::new())];
    let coordinators = [a, b];
    // A notification must never create ownership, including an ownerless Agent.
    for (coordinator, signal) in coordinators.iter().zip(&signals) {
        OwnedDiscoveryWake::new(coordinator.clone(), signal.clone())
            .committed_cancel(&agent)
            .await;
        assert!(tokio::time::timeout(Duration::ZERO, signal.notified())
            .await
            .is_err());
    }
    assert!(store.owner(&agent).await.unwrap().is_none());
    let mut owner = None;
    for (index, coordinator) in coordinators.iter().enumerate() {
        if let Some(authority) = coordinator.acquire(&agent).await.unwrap() {
            owner = Some((index, authority));
        }
    }
    let (index, authority) = owner.unwrap();
    for (coordinator, signal) in coordinators.iter().zip(&signals) {
        OwnedDiscoveryWake::new(coordinator.clone(), signal.clone())
            .committed_cancel(&agent)
            .await;
    }
    assert!(
        tokio::time::timeout(Duration::ZERO, signals[index].notified())
            .await
            .is_ok()
    );
    assert!(
        tokio::time::timeout(Duration::ZERO, signals[1 - index].notified())
            .await
            .is_err()
    );
    assert_eq!(
        store.owner(&agent).await.unwrap().unwrap().token,
        authority.token
    );
    authority.revoke();
    OwnedDiscoveryWake::new(coordinators[index].clone(), signals[index].clone())
        .committed_cancel(&agent)
        .await;
    assert!(
        tokio::time::timeout(Duration::ZERO, signals[index].notified())
            .await
            .is_err()
    );
}

#[tokio::test(start_paused = true)]
async fn draining_expired_and_failed_owner_do_not_notify() {
    for mode in ["drain", "expired", "failed"] {
        let store = Arc::new(MemoryOwnershipStore::default());
        let coordinator = register(store, "owner").await;
        let agent = AgentId("agent".into());
        coordinator.acquire(&agent).await.unwrap().unwrap();
        match mode {
            "drain" => coordinator.stop_admission(),
            "expired" => tokio::time::advance(Duration::from_secs(60)).await,
            _ => coordinator.fail().await,
        }
        let signal = Arc::new(Notify::new());
        OwnedDiscoveryWake::new(coordinator, signal.clone())
            .committed_cancel(&agent)
            .await;
        assert!(
            tokio::time::timeout(Duration::ZERO, signal.notified())
                .await
                .is_err(),
            "{mode}"
        );
    }
}
