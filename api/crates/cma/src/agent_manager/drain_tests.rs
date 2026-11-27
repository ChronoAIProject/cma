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
    let store = Arc::new(MemoryOwnershipStore::at_fixed_time(1));
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
    coordinator.stop_admission();
    (store, coordinator, authority)
}

#[tokio::test(start_paused = true)]
async fn refused_receiver_is_identified_without_releasing_its_lease() {
    let (store, coordinator, authority) = fixture().await;
    let original = store.owner(&authority.token.agent_id).await.unwrap();
    let error = OwnershipError::Unavailable("receiver acknowledgement missing".into());
    let failure = handoff(&coordinator, &authority, deadline(), || async {
        Err(error.clone())
    })
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
    let failure = handoff(&coordinator, &authority, deadline(), std::future::pending)
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
    let failure = handoff(&coordinator, &authority, deadline(), || async {
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
    handoff(&coordinator, &authority, deadline(), || async { Ok(()) })
        .await
        .unwrap();
    assert!(store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(!authority.is_current());
}

#[tokio::test(start_paused = true)]
async fn drain_retries_an_unconfirmed_receiver_and_waits_for_its_acknowledgement() {
    use std::sync::atomic::AtomicUsize;

    let (store, coordinator, authority) = fixture().await;
    let original = store.owner(&authority.token.agent_id).await.unwrap();
    let attempts = AtomicUsize::new(0);
    handoff(&coordinator, &authority, deadline(), || async {
        assert_eq!(
            store.owner(&authority.token.agent_id).await.unwrap(),
            original
        );
        assert!(!authority.is_current());
        if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
            return Err(OwnershipError::Unavailable(
                "receiver request failed".into(),
            ));
        }
        // The adapter permits a receiver reply after the two-second store deadline.
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert_eq!(
            store.owner(&authority.token.agent_id).await.unwrap(),
            original
        );
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(attempts.load(Ordering::Relaxed), 2);
    assert!(store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(coordinator
        .acquire(&authority.token.agent_id)
        .await
        .is_err());
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(23)
}
