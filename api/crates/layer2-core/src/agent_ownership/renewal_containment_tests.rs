//! Store-boundary faults exercise the real coordinator and exact-token cleanup.
use super::*;
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use test_support::MemoryOwnershipStore;

struct FaultStore {
    inner: MemoryOwnershipStore,
    mode: AtomicUsize,
    member_error: AtomicBool,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl OwnershipStore for FaultStore {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
    }
    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        if self.member_error.load(Ordering::SeqCst) {
            return Err(OwnershipError::Unavailable("membership unavailable".into()));
        }
        self.inner.heartbeat(id, draining).await
    }
    async fn membership(&self) -> Result<Membership, OwnershipError> {
        self.inner.membership().await
    }
    async fn owner(&self, id: &crate::AgentId) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.inner.owner(id).await
    }
    async fn has_owner_history(&self, id: &crate::AgentId) -> Result<bool, OwnershipError> {
        self.inner.has_owner_history(id).await
    }
    async fn acquire(
        &self,
        id: &crate::AgentId,
        member: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.inner.acquire(id, member).await
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        if token.agent_id.as_str() != "failed" {
            return self.inner.renew(token).await;
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.mode.load(Ordering::SeqCst) {
            1 => {
                tokio::time::sleep(Duration::from_millis(STORE_DEADLINE_MS - 1)).await;
                Err(OwnershipError::Unavailable("Agent ownership transaction exceeded its deadline after a known abort: WriteConflict".into()))
            }
            2 => {
                self.inner.renew(token).await?;
                Err(OwnershipError::Unavailable(
                    "commit acknowledgement unknown".into(),
                ))
            }
            3 => Err(OwnershipError::Stale),
            4 => Err(OwnershipError::IncompatibleProtocol),
            5 => {
                let mut lease = self.inner.renew(token).await?;
                lease.token.epoch += 1;
                Ok(lease)
            }
            6 => {
                let mut lease = self.inner.renew(token).await?;
                lease.expires_at_ms = 0;
                Ok(lease)
            }
            _ => self.inner.renew(token).await,
        }
    }
}

struct Receiver {
    acknowledge: AtomicBool,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "exact-receiver".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.acknowledge.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(layer3_adapter::AdapterError::Rejected(
                "acknowledgement unknown".into(),
            ))
        }
    }
}

async fn fixture(
    mode: usize,
) -> (
    Arc<FaultStore>,
    Arc<OwnershipCoordinator>,
    Arc<Authority>,
    Arc<Authority>,
    Arc<Receiver>,
) {
    let store = Arc::new(FaultStore {
        inner: MemoryOwnershipStore::default(),
        mode: AtomicUsize::new(mode),
        member_error: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    let manager = OwnershipCoordinator::register(
        store.clone(),
        Member {
            instance_id: "manager".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://manager:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 3,
        },
    )
    .await
    .unwrap();
    let failed = manager
        .acquire(&crate::AgentId("failed".into()))
        .await
        .unwrap()
        .unwrap();
    let healthy = manager
        .acquire(&crate::AgentId("healthy".into()))
        .await
        .unwrap()
        .unwrap();
    let receiver = Arc::new(Receiver {
        acknowledge: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    failed.register_receiver(receiver.clone()).unwrap();
    (store, manager, failed, healthy, receiver)
}

#[tokio::test(start_paused = true)]
async fn exhausted_owner_transaction_budget_cancels_only_its_effects_and_receiver() {
    let (store, manager, failed, healthy, _) = fixture(1).await;
    let lease = store.owner(&failed.token.agent_id).await.unwrap().unwrap();
    let effect = tokio::spawn({
        let failed = failed.clone();
        async move { failed.run(std::future::pending::<()>()).await }
    });
    let receiver_renewal = tokio::spawn({
        let failed = failed.clone();
        async move { failed.revoked().await }
    });
    tokio::time::advance(Duration::from_secs(3)).await;
    let result = manager.maintain().await;
    assert!(
        result.is_ok(),
        "one exhausted owner renewal must not stop healthy membership: {result:?}"
    );
    assert!(manager.is_ready());
    assert!(!*manager.failure_signal().borrow());
    assert!(!failed.is_current());
    assert!(failed.receiver_cancelled());
    assert!(manager.local(&failed.token.agent_id).await.is_none());
    assert_eq!(effect.await.unwrap(), Err(OwnershipError::Stale));
    receiver_renewal.await.unwrap();
    assert_eq!(
        failed.run(async { panic!("cancelled effect ran") }).await,
        Err(OwnershipError::Stale)
    );
    assert!(failed
        .renew_until(tokio::time::Instant::now() + Duration::from_secs(30))
        .is_err());
    assert!(healthy.is_current());
    assert!(healthy
        .run(async { store.check(&healthy.token).await })
        .await
        .unwrap()
        .is_ok());
    assert_eq!(
        store.owner(&failed.token.agent_id).await.unwrap().unwrap(),
        lease
    );
    for _ in 0..3 {
        manager.maintain().await.unwrap();
    }
    assert_eq!(
        store.calls.load(Ordering::SeqCst),
        1,
        "containment cannot replay a failed renewal"
    );
    assert!(manager
        .cancelled_receivers()
        .await
        .iter()
        .any(|a| Arc::ptr_eq(a, &failed)));
}

#[tokio::test(start_paused = true)]
async fn uncertain_commit_retains_actual_lease_and_never_reauthorizes_same_epoch() {
    let (store, manager, failed, healthy, _) = fixture(2).await;
    let original = store.owner(&failed.token.agent_id).await.unwrap().unwrap();
    tokio::time::advance(Duration::from_secs(3)).await;
    manager.maintain().await.unwrap();
    let actual = store.owner(&failed.token.agent_id).await.unwrap().unwrap();
    assert!(actual.expires_at_ms > original.expires_at_ms);
    assert_eq!(actual.token, original.token);
    assert!(manager
        .acquire(&failed.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(!manager.forget_cancelled(&failed).await.unwrap());
    manager.maintain().await.unwrap();
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.owner(&failed.token.agent_id).await.unwrap().unwrap(),
        actual
    );
    assert!(healthy.is_current());
    assert!(!failed.is_current());
}

#[tokio::test(start_paused = true)]
async fn refused_cleanup_retains_custody_until_ack_then_recovers_without_capacity_leak() {
    let (store, manager, failed, healthy, receiver) = fixture(1).await;
    manager.maintain().await.unwrap();
    let lease = store.owner(&failed.token.agent_id).await.unwrap().unwrap();
    assert!(manager
        .handoff(&failed, failed.revoke_transports())
        .await
        .is_err());
    assert_eq!(
        store.owner(&failed.token.agent_id).await.unwrap().unwrap(),
        lease
    );
    assert!(!manager.forget_cancelled(&failed).await.unwrap());
    manager.maintain().await.unwrap();
    assert!(healthy.is_current());
    assert_eq!(manager.cancelled_receivers().await.len(), 1);
    receiver.acknowledge.store(true, Ordering::SeqCst);
    manager
        .handoff(&failed, failed.revoke_transports())
        .await
        .unwrap();
    assert!(store.owner(&failed.token.agent_id).await.unwrap().is_none());
    assert_eq!(store.membership().await.unwrap().members[0].owned_agents, 1);
    store.mode.store(0, Ordering::SeqCst);
    let successor = manager
        .acquire(&failed.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert!(successor.token.epoch > failed.token.epoch);
    manager
        .handoff(&failed, failed.revoke_transports())
        .await
        .unwrap();
    assert_eq!(
        store
            .owner(&successor.token.agent_id)
            .await
            .unwrap()
            .unwrap()
            .token,
        successor.token
    );
    assert_eq!(store.membership().await.unwrap().members[0].owned_agents, 2);
    assert!(successor.is_current());
    assert_eq!(receiver.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn expiry_takeover_preserves_successor_and_does_not_leak_accounting() {
    let (store, manager, failed, healthy, _) = fixture(1).await;
    manager.maintain().await.unwrap();
    for _ in 0..5 {
        tokio::time::advance(Duration::from_secs(3)).await;
        manager.maintain().await.unwrap();
    }
    assert!(store.owner(&failed.token.agent_id).await.unwrap().is_none());
    assert!(
        !manager.forget_cancelled(&failed).await.unwrap(),
        "absence alone is not finality"
    );
    store.mode.store(0, Ordering::SeqCst);
    let successor = manager
        .acquire(&failed.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert!(successor.token.epoch > failed.token.epoch);
    assert!(manager.forget_cancelled(&failed).await.unwrap());
    assert!(Arc::ptr_eq(
        &manager.local(&successor.token.agent_id).await.unwrap(),
        &successor
    ));
    assert_eq!(store.membership().await.unwrap().members[0].owned_agents, 2);
    assert!(healthy.is_current());
    assert!(!failed.is_current());
}

#[tokio::test(start_paused = true)]
async fn member_loss_and_expired_or_replaced_owner_and_protocol_still_fail_process() {
    for mode in [0, 3, 4, 5, 6] {
        let (store, manager, failed, healthy, _) = fixture(mode).await;
        if mode == 0 {
            store.member_error.store(true, Ordering::SeqCst);
        }
        assert!(manager.maintain().await.is_err(), "mode {mode}");
        assert!(!manager.is_ready());
        assert!(*manager.failure_signal().borrow());
        assert!(!failed.is_current());
        assert!(!healthy.is_current());
    }
}

#[tokio::test(start_paused = true)]
async fn unavailable_after_local_owner_expiry_is_not_relabelled_as_contained() {
    let (_, manager, failed, healthy, _) = fixture(1).await;
    tokio::time::advance(Duration::from_millis(DEFAULT_LEASE_MS - 1000)).await;
    assert!(manager.maintain().await.is_err());
    assert!(!failed.receiver_cancelled());
    assert!(!healthy.is_current());
    assert!(*manager.failure_signal().borrow());
}

#[tokio::test(start_paused = true)]
async fn expired_member_deadline_is_fatal_before_any_renewal() {
    let (store, manager, _, healthy, _) = fixture(1).await;
    tokio::time::advance(Duration::from_millis(DEFAULT_LEASE_MS)).await;
    assert!(manager.maintain().await.is_err());
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
    assert!(!healthy.is_current());
    assert!(*manager.failure_signal().borrow());
}
