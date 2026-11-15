use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipStore, DEFAULT_MAX_OWNED_AGENTS,
    PROTOCOL_VERSION,
};
use layer2_core::AgentId;

async fn fixture() -> (
    Arc<MemoryOwnershipStore>,
    Arc<OwnershipCoordinator>,
    Arc<Authority>,
) {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::register(
        store.clone(),
        Member {
            instance_id: "manager".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://manager:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
        },
    )
    .await
    .unwrap();
    let authority = coordinator
        .acquire(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    (store, coordinator, authority)
}

#[tokio::test(start_paused = true)]
async fn refused_receiver_is_identified_without_releasing_its_lease() {
    let (store, coordinator, authority) = fixture().await;
    let original = store.owner(&authority.token.agent_id).await.unwrap();
    let error = OwnershipError::Unavailable("receiver acknowledgement missing".into());
    let failure = handoff(&coordinator, &authority, async { Err(error.clone()) })
        .await
        .unwrap_err();
    assert_eq!(failure.phase, "receiver_revocation");
    assert_eq!(failure.error, error);
    assert!(!authority.is_current());
    assert_eq!(
        store.owner(&authority.token.agent_id).await.unwrap(),
        original
    );
}

#[tokio::test(start_paused = true)]
async fn receiver_timeout_is_identified_without_releasing_its_lease() {
    let (store, coordinator, authority) = fixture().await;
    let original = store.owner(&authority.token.agent_id).await.unwrap();
    let failure = handoff(&coordinator, &authority, std::future::pending())
        .await
        .unwrap_err();
    assert_eq!(failure.phase, "receiver_revocation");
    assert!(matches!(failure.error, OwnershipError::Unavailable(_)));
    assert!(!authority.is_current());
    assert_eq!(
        store.owner(&authority.token.agent_id).await.unwrap(),
        original
    );
}

#[tokio::test(start_paused = true)]
async fn release_failure_is_distinct_and_retry_does_not_revive_authority() {
    let (store, coordinator, authority) = fixture().await;
    let original = store.owner(&authority.token.agent_id).await.unwrap();
    let failure = handoff(&coordinator, &authority, async {
        store.partition(true);
        Ok(())
    })
    .await
    .unwrap_err();
    assert_eq!(failure.phase, "durable_release");
    assert_eq!(
        failure.error,
        OwnershipError::Unavailable("synthetic store partition".into())
    );
    store.partition(false);
    assert_eq!(
        store.owner(&authority.token.agent_id).await.unwrap(),
        original
    );
    assert!(!authority.is_current());
    handoff(&coordinator, &authority, async { Ok(()) })
        .await
        .unwrap();
    assert!(store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(!authority.is_current());
}
