use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipStore, DEFAULT_MAX_OWNED_AGENTS,
    PROTOCOL_VERSION,
};
use std::{collections::BTreeSet, time::Duration};

async fn coordinator(store: Arc<MemoryOwnershipStore>) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store,
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
    .unwrap()
}

fn unavailable() -> OwnershipError {
    OwnershipError::Unavailable("receiver acknowledgement missing".into())
}

/// The production failure: revocation at the runtime is not confirmed.
async fn failed_handoff(
    coordinator: &OwnershipCoordinator,
    authority: &Arc<Authority>,
) -> OwnershipError {
    coordinator
        .handoff(authority, async { Err(unavailable()) })
        .await
        .unwrap_err()
}

#[tokio::test(start_paused = true)]
async fn disabled_or_non_transient_failures_still_stop_the_process() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = coordinator(store.clone()).await;
    let authority = coordinator
        .acquire(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let contained = ContainedHandoffs::default();
    assert_eq!(
        contained.contain("current_handoff", &authority, unavailable()),
        Err(unavailable()),
        "off by default"
    );
    contained.enable();
    for error in [
        OwnershipError::Stale,
        OwnershipError::IncompatibleProtocol,
        OwnershipError::Invalid("record".into()),
    ] {
        assert_eq!(
            contained.contain("current_handoff", &authority, error.clone()),
            Err(error)
        );
    }
    assert_eq!(contained.pending(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_contained_receiver_stays_fenced_until_a_confirmed_retry_releases_it() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = coordinator(store.clone()).await;
    let healthy = coordinator
        .acquire(&AgentId("healthy".into()))
        .await
        .unwrap()
        .unwrap();
    let failing = coordinator
        .acquire(&AgentId("failing".into()))
        .await
        .unwrap()
        .unwrap();
    let original = store.owner(&failing.token.agent_id).await.unwrap();
    let contained = ContainedHandoffs::default();
    contained.enable();
    let error = failed_handoff(&coordinator, &failing).await;
    contained
        .contain("current_handoff", &failing, error)
        .unwrap();
    assert_eq!(contained.pending(), 1);
    // Fenced: no local authority, and the durable lease is not released.
    assert!(!failing.is_current());
    assert_eq!(
        store.owner(&failing.token.agent_id).await.unwrap(),
        original
    );
    assert!(healthy.is_current());

    // A retry that still cannot confirm keeps the receiver contained.
    let completed = contained
        .retry(&coordinator, |authority| {
            let coordinator = coordinator.clone();
            async move {
                coordinator
                    .handoff(&authority, async { Err(unavailable()) })
                    .await
                    .map(|()| true)
            }
        })
        .await
        .unwrap();
    assert!(completed.is_empty());
    assert_eq!(contained.pending(), 1);
    assert_eq!(
        store.owner(&failing.token.agent_id).await.unwrap(),
        original
    );

    // A confirmed retry releases the exact lease and drops the entry.
    let completed = contained
        .retry(&coordinator, |authority| {
            let coordinator = coordinator.clone();
            async move {
                coordinator
                    .handoff(&authority, async { Ok(()) })
                    .await
                    .map(|()| true)
            }
        })
        .await
        .unwrap();
    assert_eq!(completed, vec![failing.token.agent_id.clone()]);
    assert_eq!(contained.pending(), 0);
    assert!(store
        .owner(&failing.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(!failing.is_current());
    assert!(healthy.is_current());
}

#[tokio::test(start_paused = true)]
async fn a_fatal_retry_error_still_stops_the_process() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = coordinator(store).await;
    let failing = coordinator
        .acquire(&AgentId("failing".into()))
        .await
        .unwrap()
        .unwrap();
    let contained = ContainedHandoffs::default();
    contained.enable();
    let error = failed_handoff(&coordinator, &failing).await;
    contained
        .contain("inactive_retirement", &failing, error)
        .unwrap();
    let result = contained
        .retry(&coordinator, |_| async {
            Err(OwnershipError::IncompatibleProtocol)
        })
        .await;
    assert_eq!(result, Err(OwnershipError::IncompatibleProtocol));
    assert_eq!(contained.pending(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_entry_the_coordinator_no_longer_owns_is_dropped_without_retry() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = coordinator(store).await;
    let failing = coordinator
        .acquire(&AgentId("failing".into()))
        .await
        .unwrap()
        .unwrap();
    let contained = ContainedHandoffs::default();
    contained.enable();
    let error = failed_handoff(&coordinator, &failing).await;
    contained
        .contain("current_handoff", &failing, error)
        .unwrap();
    // Another path completed the release; the coordinator dropped the authority.
    coordinator
        .handoff(&failing, async { Ok(()) })
        .await
        .unwrap();
    let completed = contained
        .retry(&coordinator, |_| async {
            panic!("a released authority must not be retried")
        })
        .await
        .unwrap();
    assert_eq!(completed, vec![failing.token.agent_id.clone()]);
    assert_eq!(contained.pending(), 0);
}

#[tokio::test(start_paused = true)]
async fn persistently_failing_receivers_do_not_starve_the_rest() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = coordinator(store).await;
    let contained = ContainedHandoffs::default();
    contained.enable();
    for index in 0..6 {
        let authority = coordinator
            .acquire(&AgentId(format!("failing-{index}")))
            .await
            .unwrap()
            .unwrap();
        let error = failed_handoff(&coordinator, &authority).await;
        contained
            .contain("current_handoff", &authority, error)
            .unwrap();
    }
    let attempted = Arc::new(Mutex::new(BTreeSet::new()));
    for _ in 0..2 {
        contained
            .retry(&coordinator, |authority| {
                let attempted = attempted.clone();
                async move {
                    attempted
                        .lock()
                        .unwrap()
                        .insert(authority.token.agent_id.clone());
                    Err(unavailable())
                }
            })
            .await
            .unwrap();
    }
    assert_eq!(attempted.lock().unwrap().len(), 6);
    assert_eq!(contained.pending(), 6);
}

#[tokio::test(start_paused = true)]
async fn an_expired_contained_lease_is_forgotten_so_this_process_can_acquire_again() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = coordinator(store.clone()).await;
    let failing = coordinator
        .acquire(&AgentId("failing".into()))
        .await
        .unwrap()
        .unwrap();
    let contained = ContainedHandoffs::default();
    contained.enable();
    let error = failed_handoff(&coordinator, &failing).await;
    contained
        .contain("current_handoff", &failing, error)
        .unwrap();
    // While the contained token still holds its lease, this process refuses
    // a new acquisition, exactly as during any unfinished handoff.
    assert!(matches!(
        coordinator.acquire(&failing.token.agent_id).await,
        Err(OwnershipError::Stale)
    ));
    // Membership stays live; the retiring owner lease is never renewed.
    for _ in 0..4 {
        tokio::time::advance(Duration::from_secs(4)).await;
        coordinator.maintain().await.unwrap();
    }
    assert!(store
        .owner(&failing.token.agent_id)
        .await
        .unwrap()
        .is_none());
    let completed = contained
        .retry(&coordinator, |_| async {
            panic!("an expired token is forgotten before any retry")
        })
        .await
        .unwrap();
    assert_eq!(completed, vec![failing.token.agent_id.clone()]);
    assert_eq!(contained.pending(), 0);
    let replacement = coordinator
        .acquire(&failing.token.agent_id)
        .await
        .unwrap()
        .expect("the Agent can be owned again after its lease expired");
    assert!(replacement.token.epoch > failing.token.epoch);
    assert!(replacement.is_current());
    assert!(!failing.is_current());
}
