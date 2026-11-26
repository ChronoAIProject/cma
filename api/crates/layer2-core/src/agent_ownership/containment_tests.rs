use super::test_support::MemoryOwnershipStore;
use super::*;
use std::sync::Arc;

async fn manager(policy: MaintenancePolicy) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register_with_policy(
        Arc::new(MemoryOwnershipStore::default()),
        Member {
            instance_id: "manager".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://manager.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
        },
        false,
        policy,
    )
    .await
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn failed_retirement_is_selected_again_and_defers_only_its_agent() {
    let coordinator = manager(MaintenancePolicy::ContainFailures).await;
    let authority = coordinator
        .acquire(&AgentId("retiring".into()))
        .await
        .unwrap()
        .unwrap();
    let healthy = coordinator
        .acquire(&AgentId("healthy".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        coordinator
            .handoff(&authority, async {
                Err(OwnershipError::Unavailable(
                    "synthetic receiver unavailable".into(),
                ))
            })
            .await,
        Err(OwnershipError::Unavailable(_))
    ));
    assert!(!authority.is_current());
    coordinator.maintain().await.unwrap();
    assert!(coordinator.is_ready());
    assert!(healthy.is_current());
    let pending = coordinator.cancelled_receivers().await;
    assert!(
        pending.iter().any(|entry| Arc::ptr_eq(entry, &authority)),
        "a failed retirement must remain discoverable after it stops being current"
    );
    assert!(coordinator
        .acquire(&authority.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(Arc::ptr_eq(
        &coordinator
            .acquire(&healthy.token.agent_id)
            .await
            .unwrap()
            .unwrap(),
        &healthy
    ));
    coordinator.handoff_background(&authority).await.unwrap();
    let successor = coordinator
        .acquire(&authority.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert!(successor.token.epoch > authority.token.epoch);
    assert!(successor.is_current());
    assert!(!authority.is_current());
}

#[tokio::test(start_paused = true)]
async fn legacy_retirement_keeps_its_existing_deferral_error() {
    let coordinator = manager(MaintenancePolicy::Legacy).await;
    let authority = coordinator
        .acquire(&AgentId("retiring".into()))
        .await
        .unwrap()
        .unwrap();
    let _ = coordinator
        .handoff(&authority, async {
            Err(OwnershipError::Unavailable("synthetic failure".into()))
        })
        .await;
    assert!(coordinator.cancelled_receivers().await.is_empty());
    assert!(matches!(
        coordinator.acquire(&authority.token.agent_id).await,
        Err(OwnershipError::Stale)
    ));
}
