use super::test_support::MemoryOwnershipStore;
use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Notify;

struct PausedRenew {
    inner: Arc<MemoryOwnershipStore>,
    entered: Notify,
    resume: Notify,
    before_pause: bool,
    pause_acquire: std::sync::atomic::AtomicBool,
    pause_owner: bool,
}

#[async_trait::async_trait]
impl OwnershipStore for PausedRenew {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
    }
    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        self.inner.heartbeat(id, draining).await
    }
    async fn membership(&self) -> Result<Membership, OwnershipError> {
        self.inner.membership().await
    }
    async fn owner(&self, id: &AgentId) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.inner.owner(id).await
    }
    async fn reconciliation_owners(
        &self,
        ids: &[&AgentId],
    ) -> Result<Vec<ReconciliationOwner>, OwnershipError> {
        if self.pause_owner {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        self.inner.reconciliation_owners(ids).await
    }
    async fn has_owner_history(&self, id: &AgentId) -> Result<bool, OwnershipError> {
        self.inner.has_owner_history(id).await
    }
    async fn acquire(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        let acquired = if self.before_pause {
            Some(self.inner.acquire(id, instance).await)
        } else {
            None
        };
        if self.pause_acquire.load(Ordering::Acquire) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        match acquired {
            Some(result) => result,
            None => self.inner.acquire(id, instance).await,
        }
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        if self.pause_acquire.load(Ordering::Acquire) {
            return self.inner.renew(token).await;
        }
        let result = if self.before_pause {
            Some(self.inner.renew(token).await)
        } else {
            None
        };
        self.entered.notify_one();
        self.resume.notified().await;
        match result {
            Some(value) => value,
            None => self.inner.renew(token).await,
        }
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.release(token).await
    }
    async fn record_receiver_failure(
        &self,
        token: &OwnershipToken,
        failure: OwnershipFailure,
    ) -> Result<(), OwnershipError> {
        self.inner.record_receiver_failure(token, failure).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

async fn start(store: Arc<dyn OwnershipStore>) -> Arc<OwnershipCoordinator> {
    start_with_policy(store, MaintenancePolicy::Legacy).await
}

async fn start_with_policy(
    store: Arc<dyn OwnershipStore>,
    policy: MaintenancePolicy,
) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register_with_policy(
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
        false,
        policy,
    )
    .await
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn late_success_or_stale_renewal_after_handoff_cannot_revive_or_fail_the_manager() {
    for before_pause in [false, true] {
        let store = Arc::new(PausedRenew {
            inner: Arc::new(MemoryOwnershipStore::default()),
            entered: Notify::new(),
            resume: Notify::new(),
            before_pause,
            pause_acquire: std::sync::atomic::AtomicBool::new(false),
            pause_owner: false,
        });
        let coordinator = start(store.clone()).await;
        let authority = coordinator
            .acquire(&AgentId("agent".into()))
            .await
            .unwrap()
            .unwrap();
        let renewing = tokio::spawn({
            let coordinator = coordinator.clone();
            async move { coordinator.maintain().await }
        });
        store.entered.notified().await;
        coordinator
            .handoff(&authority, async { Ok(()) })
            .await
            .unwrap();
        store.resume.notify_one();
        assert!(renewing.await.unwrap().unwrap().is_empty());
        assert!(coordinator.is_ready());
        assert!(!authority.is_current());
        assert!(coordinator.owned().await.is_empty());
        assert!(store
            .inner
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .is_none());
        let successor = coordinator
            .acquire(&authority.token.agent_id)
            .await
            .unwrap()
            .unwrap();
        assert!(successor.token.epoch > authority.token.epoch);
    }
}

#[tokio::test(start_paused = true)]
async fn concurrent_handoffs_confirm_and_release_the_exact_authority_only_once() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = start(store.clone()).await;
    let authority = coordinator
        .acquire(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let calls = AtomicUsize::new(0);
    let revoke = || async {
        calls.fetch_add(1, Ordering::Relaxed);
        tokio::task::yield_now().await;
        Ok(())
    };
    let (first, second) = tokio::join!(
        coordinator.handoff(&authority, revoke()),
        coordinator.handoff(&authority, revoke())
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(!authority.is_current());
}

#[tokio::test(start_paused = true)]
async fn uncertain_receiver_revoke_retains_original_expiry_without_renewing_retired_authority() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = start(store.clone()).await;
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
    let original = store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert!(coordinator
        .handoff(&authority, async {
            Err(OwnershipError::Unavailable("receiver reply lost".into()))
        })
        .await
        .is_err());
    tokio::time::advance(std::time::Duration::from_millis(HEARTBEAT_INTERVAL_MS)).await;
    coordinator.maintain().await.unwrap();
    assert_eq!(
        store
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .unwrap(),
        original
    );
    assert!(!authority.is_current());
    assert!(healthy.is_current());
    assert!(coordinator.is_ready());
}

#[tokio::test(start_paused = true)]
async fn slow_acquisition_cannot_block_renewal_and_drain_takes_custody_of_it() {
    let store = Arc::new(PausedRenew {
        inner: Arc::new(MemoryOwnershipStore::default()),
        entered: Notify::new(),
        resume: Notify::new(),
        before_pause: true,
        pause_acquire: std::sync::atomic::AtomicBool::new(false),
        pause_owner: false,
    });
    let coordinator = start(store.clone()).await;
    let healthy = coordinator
        .acquire(&AgentId("healthy".into()))
        .await
        .unwrap()
        .unwrap();
    store.pause_acquire.store(true, Ordering::Release);
    let acquiring = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.acquire(&AgentId("slow".into())).await }
    });
    store.entered.notified().await;
    // Renewal completes while the other acquisition's store response is held.
    coordinator.maintain().await.unwrap();
    assert!(healthy.is_current());
    // Drain closes admission at once, then joins the held acquisition so the
    // lease it committed is drained instead of being left to expire.
    let draining = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.begin_drain().await }
    });
    tokio::task::yield_now().await;
    assert!(coordinator.is_draining());
    store.resume.notify_one();
    assert!(matches!(
        acquiring.await.unwrap(),
        Err(OwnershipError::Stale)
    ));
    let owned = draining.await.unwrap();
    assert_eq!(owned.len(), 2);
    assert!(owned.iter().all(|authority| !authority.is_current()));
    assert!(!healthy.is_current());
    let slow = owned
        .iter()
        .find(|authority| authority.token.agent_id == AgentId("slow".into()))
        .unwrap();
    coordinator.handoff(slow, async { Ok(()) }).await.unwrap();
    assert!(store
        .inner
        .owner(&AgentId("slow".into()))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(start_paused = true)]
async fn reconciliation_owner_lookup_times_out_without_serializing_acquisition() {
    let store = Arc::new(PausedRenew {
        inner: Arc::new(MemoryOwnershipStore::default()),
        entered: Notify::new(),
        resume: Notify::new(),
        before_pause: false,
        pause_acquire: std::sync::atomic::AtomicBool::new(false),
        pause_owner: true,
    });
    let coordinator = start(store.clone()).await;
    let candidate = AgentId("candidate".into());
    let started = tokio::time::Instant::now();
    let observed = tokio::spawn({
        let coordinator = coordinator.clone();
        let candidate = candidate.clone();
        async move {
            let mut owners = coordinator
                .observe_foreign_reconciliation_owners([&candidate])
                .await;
            coordinator.defer_reconciliation_for_foreign_owner(&mut owners, &candidate)
        }
    });
    store.entered.notified().await;
    let healthy = coordinator
        .acquire(&AgentId("healthy".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(healthy.is_current());
    assert_eq!(started.elapsed(), std::time::Duration::ZERO);
    assert!(!observed.await.unwrap());
    assert_eq!(started.elapsed(), std::time::Duration::from_millis(200));
    let local = coordinator.acquire(&candidate).await.unwrap().unwrap();

    // A cancelled local receiver with an unexpired same-instance lease must
    // take the ordinary acquisition path and cannot revive its old epoch.
    local.cancel_receiver();
    let started = tokio::time::Instant::now();
    let mut owners = coordinator
        .observe_foreign_reconciliation_owners([&candidate])
        .await;
    assert!(!coordinator.defer_reconciliation_for_foreign_owner(&mut owners, &candidate));
    assert_eq!(started.elapsed(), std::time::Duration::ZERO);
    assert!(coordinator.acquire(&candidate).await.unwrap().is_none());
    assert!(!local.is_current());
}

#[tokio::test(start_paused = true)]
async fn cancelled_acquisition_caller_cannot_hide_a_committed_write_from_drain() {
    let store = Arc::new(PausedRenew {
        inner: Arc::new(MemoryOwnershipStore::default()),
        entered: Notify::new(),
        resume: Notify::new(),
        before_pause: true,
        pause_acquire: std::sync::atomic::AtomicBool::new(true),
        pause_owner: false,
    });
    let coordinator = start_with_policy(store.clone(), MaintenancePolicy::ContainFailures).await;
    let id = AgentId("committed-unreturned".into());
    let caller = tokio::spawn({
        let coordinator = coordinator.clone();
        let id = id.clone();
        async move { coordinator.acquire(&id).await }
    });
    store.entered.notified().await;
    assert!(store.inner.owner(&id).await.unwrap().is_some());
    caller.abort();
    assert!(caller
        .await
        .err()
        .expect("caller was cancelled")
        .is_cancelled());
    store.resume.notify_one();
    let custody = coordinator.begin_drain().await;
    assert_eq!(
        custody.len(),
        1,
        "drain must retain the committed result after its caller disappears"
    );
    assert_eq!(custody[0].token.agent_id, id);
    assert!(!custody[0].is_current());
}

#[path = "coordinator_custody_tests.rs"]
mod custody;
