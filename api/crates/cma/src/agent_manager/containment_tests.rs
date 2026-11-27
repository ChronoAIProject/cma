use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipStore, DEFAULT_MAX_OWNED_AGENTS,
    PROTOCOL_VERSION,
};
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use std::sync::atomic::AtomicBool;
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
) -> BackgroundHandoffError {
    coordinator
        .handoff(authority, async { Err(unavailable()) })
        .await
        .map_err(|error| match error {
            OwnershipError::Unavailable(detail) => {
                BackgroundHandoffError::ReceiverUnavailable(detail)
            }
            error => BackgroundHandoffError::Ownership(error),
        })
        .unwrap_err()
}

#[tokio::test(start_paused = true)]
async fn ownership_errors_respect_the_existing_release_policy() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = coordinator(store.clone()).await;
    let authority = coordinator
        .acquire(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let contained = ContainedHandoffs::default();
    assert_eq!(
        contained.contain(
            "current_handoff",
            &authority,
            BackgroundHandoffError::Ownership(unavailable()),
        ),
        Err(unavailable()),
        "off by default"
    );
    for error in [
        OwnershipError::Stale,
        OwnershipError::IncompatibleProtocol,
        OwnershipError::Invalid("record".into()),
    ] {
        assert_eq!(
            contained.contain(
                "current_handoff",
                &authority,
                BackgroundHandoffError::Ownership(error.clone()),
            ),
            Err(error)
        );
    }
    assert_eq!(contained.pending(), 0);
    contained.enable();
    contained
        .contain(
            "current_handoff",
            &authority,
            BackgroundHandoffError::Ownership(unavailable()),
        )
        .unwrap();
    assert_eq!(contained.pending(), 1);
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
            async move { Err(failed_handoff(&coordinator, &authority).await) }
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
                    .map_err(BackgroundHandoffError::Ownership)
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
async fn acknowledged_retry_respects_release_failure_policy() {
    struct Receiver {
        acknowledge: AtomicBool,
        store: Arc<MemoryOwnershipStore>,
    }

    #[async_trait::async_trait]
    impl ReceiverRevocation for Receiver {
        fn identity(&self) -> String {
            "release-policy-receiver".into()
        }

        async fn revoke(&self) -> layer3_adapter::Result<()> {
            if self.acknowledge.load(Ordering::Acquire) {
                // The retry already read custody. Fail its durable release.
                self.store.partition(true);
                Ok(())
            } else {
                Err(layer3_adapter::AdapterError::Unavailable(
                    "synthetic receiver refusal".into(),
                ))
            }
        }
    }

    for contain_release_failures in [false, true] {
        let store = Arc::new(MemoryOwnershipStore::default());
        let coordinator = coordinator(store.clone()).await;
        let id = AgentId("failing".into());
        let authority = coordinator.acquire(&id).await.unwrap().unwrap();
        let receiver = Arc::new(Receiver {
            acknowledge: AtomicBool::new(false),
            store: store.clone(),
        });
        authority.register_receiver(receiver.clone()).unwrap();
        authority.receiver_confirmed(&receiver.identity());
        let contained = ContainedHandoffs::default();
        if contain_release_failures {
            contained.enable();
        }

        let error = coordinator
            .handoff_background_classified(&authority)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            BackgroundHandoffError::ReceiverUnavailable(_)
        ));
        contained
            .contain("current_handoff", &authority, error)
            .unwrap();
        assert!(!authority.is_current());
        receiver.acknowledge.store(true, Ordering::Release);

        let retry = |authority| {
            let coordinator = coordinator.clone();
            async move { coordinator.handoff_background_classified(&authority).await }
        };
        let result = contained.retry(&coordinator, retry).await;
        if contain_release_failures {
            assert!(result.unwrap().is_empty());
        } else {
            assert_eq!(
                result,
                Err(OwnershipError::Unavailable(
                    "synthetic store partition".into()
                ))
            );
        }
        assert_eq!(contained.pending(), 1);
        store.partition(false);
        assert_eq!(
            store.owner(&id).await.unwrap().unwrap().token,
            authority.token
        );
        assert_eq!(
            contained.retry(&coordinator, retry).await.unwrap(),
            vec![id.clone()]
        );
        assert_eq!(contained.pending(), 0);
        assert!(store.owner(&id).await.unwrap().is_none());
    }
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
    let error = failed_handoff(&coordinator, &failing).await;
    contained
        .contain("inactive_retirement", &failing, error)
        .unwrap();
    let result = contained
        .retry(&coordinator, |_| async {
            Err(BackgroundHandoffError::Ownership(
                OwnershipError::IncompatibleProtocol,
            ))
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
                    Err(BackgroundHandoffError::ReceiverUnavailable(
                        "receiver acknowledgement missing".into(),
                    ))
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
