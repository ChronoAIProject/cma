use super::*;
use std::time::Duration;

#[tokio::test(start_paused = true)]
async fn cancelled_and_timed_out_committed_acquisitions_remain_accounted_without_release() {
    for cancel in [true, false] {
        let store = Arc::new(PausedRenew {
            inner: Arc::new(MemoryOwnershipStore::default()),
            entered: Notify::new(),
            resume: Notify::new(),
            before_pause: false,
            pause_acquire: std::sync::atomic::AtomicBool::new(true),
        });
        let coordinator = start(store.clone()).await;
        let id = AgentId("committed-hidden-reply".into());
        let acquiring = tokio::spawn({
            let coordinator = coordinator.clone();
            let id = id.clone();
            async move { coordinator.acquire(&id).await }
        });
        store.entered.notified().await;
        let original = store.owner(&id).await.unwrap().unwrap();
        assert_eq!(coordinator.uncertain_acquisitions(), 1);
        if cancel {
            acquiring.abort();
            assert!(matches!(acquiring.await, Err(error) if error.is_cancelled()));
        } else {
            tokio::time::advance(Duration::from_millis(STORE_DEADLINE_MS + 1)).await;
            assert!(matches!(
                acquiring.await.unwrap(),
                Err(OwnershipError::Unavailable(_))
            ));
        }
        store.pause_acquire.store(false, Ordering::Release);
        assert!(coordinator.begin_drain().await.is_empty());
        assert_eq!(coordinator.uncertain_acquisitions(), 1);
        assert_eq!(store.owner(&id).await.unwrap(), Some(original.clone()));
        assert!(coordinator.local(&id).await.is_none());
        // No exact token was published or invented for release. The store's
        // actual expiry permits one successor with a strictly newer epoch.
        tokio::time::advance(Duration::from_millis(DEFAULT_LEASE_MS + 1)).await;
        let successor = OwnershipCoordinator::register(
            store.inner.clone(),
            Member {
                instance_id: "successor".into(),
                protocol: PROTOCOL_VERSION,
                relay_address: "http://successor:8080".into(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 256,
            },
        )
        .await
        .unwrap();
        let authority = successor.acquire(&id).await.unwrap().unwrap();
        assert!(authority.token.epoch > original.token.epoch);
        assert!(store.check(&original.token).await.is_err());
        assert!(authority.is_current());
    }
}

#[tokio::test(start_paused = true)]
async fn confirmed_fenced_acquisition_resolves_prior_unknown_without_a_second_epoch() {
    let store = Arc::new(PausedRenew {
        inner: Arc::new(MemoryOwnershipStore::default()),
        entered: Notify::new(),
        resume: Notify::new(),
        before_pause: false,
        pause_acquire: std::sync::atomic::AtomicBool::new(true),
    });
    let coordinator = start(store.clone()).await;
    let id = AgentId("confirmed-retry".into());
    let acquiring = tokio::spawn({
        let coordinator = coordinator.clone();
        let id = id.clone();
        async move { coordinator.acquire(&id).await }
    });
    store.entered.notified().await;
    let original = store.owner(&id).await.unwrap().unwrap();
    acquiring.abort();
    assert!(matches!(acquiring.await, Err(error) if error.is_cancelled()));
    assert_eq!(coordinator.uncertain_acquisitions(), 1);
    store.pause_acquire.store(false, Ordering::Release);
    let authority = coordinator.acquire(&id).await.unwrap().unwrap();
    assert_eq!(authority.token, original.token);
    assert!(authority.is_current());
    assert_eq!(coordinator.uncertain_acquisitions(), 0);
    assert_eq!(coordinator.begin_drain().await.len(), 1);
    coordinator
        .handoff(&authority, authority.revoke_transports())
        .await
        .unwrap();
    assert!(store.owner(&id).await.unwrap().is_none());
}

#[tokio::test(start_paused = true)]
async fn readiness_loss_before_acquire_reply_retains_cancelled_exact_custody() {
    let store = Arc::new(PausedRenew {
        inner: Arc::new(MemoryOwnershipStore::default()),
        entered: Notify::new(),
        resume: Notify::new(),
        before_pause: false,
        pause_acquire: std::sync::atomic::AtomicBool::new(true),
    });
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
    let ready = Arc::new(std::sync::atomic::AtomicBool::new(true));
    coordinator
        .set_admission_readiness({
            let ready = ready.clone();
            Arc::new(move || ready.load(Ordering::Acquire))
        })
        .unwrap();
    coordinator.activate().await.unwrap();
    let acquiring = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.acquire(&AgentId("late".into())).await }
    });
    store.entered.notified().await;
    ready.store(false, Ordering::Release);
    store.resume.notify_one();
    assert!(matches!(
        acquiring.await.unwrap(),
        Err(OwnershipError::Stale)
    ));
    let owned = coordinator.owned().await;
    assert_eq!(owned.len(), 1);
    assert!(!owned[0].is_current());
    assert_eq!(coordinator.uncertain_acquisitions(), 0);
    let original = store.owner(&owned[0].token.agent_id).await.unwrap();
    assert!(coordinator
        .handoff(&owned[0], async {
            Err(OwnershipError::Unavailable("unconfirmed receiver".into()))
        })
        .await
        .is_err());
    assert_eq!(
        store.owner(&owned[0].token.agent_id).await.unwrap(),
        original
    );
}
