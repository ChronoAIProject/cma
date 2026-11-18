//! Actual majority writes with held replies at the coordinator boundary.
use super::*;
use layer2_core::agent_ownership::OwnershipCoordinator;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Notify;

struct Held {
    inner: MongoAgentOwnership,
    operation: AtomicUsize,
    entered: Notify,
    resume: Notify,
}

impl Held {
    async fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: fixture().await,
            operation: AtomicUsize::new(0),
            entered: Notify::new(),
            resume: Notify::new(),
        })
    }

    async fn reply<T>(&self, operation: usize, result: T) -> T {
        if self.operation.load(Ordering::Acquire) == operation {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        result
    }
}

#[async_trait::async_trait]
impl OwnershipStore for Held {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.reply(1, self.inner.register(member).await).await
    }
    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        let result = self.inner.heartbeat(id, draining).await;
        if draining {
            result
        } else {
            self.reply(2, result).await
        }
    }
    async fn membership(&self) -> Result<Membership, OwnershipError> {
        self.inner.membership().await
    }
    async fn owner(&self, id: &AgentId) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.inner.owner(id).await
    }
    async fn has_owner_history(&self, id: &AgentId) -> Result<bool, OwnershipError> {
        self.inner.has_owner_history(id).await
    }
    async fn acquire(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.reply(3, self.inner.acquire(id, instance).await).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        self.reply(4, self.inner.renew(token).await).await
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn dormant_membership_and_late_committed_acquisition_have_exact_drain_custody() {
    let store = Held::new().await;
    let manager = OwnershipCoordinator::prepare(store.clone(), member("manager"));
    let id = AgentId("late-committed".into());
    assert!(manager.maintain().await.unwrap().is_empty());
    assert!(manager.acquire(&id).await.is_err());
    assert!(store.membership().await.unwrap().members.is_empty());
    assert!(!store.has_owner_history(&id).await.unwrap());
    manager.activate().await.unwrap();
    store.operation.store(3, Ordering::Release);
    let acquiring = tokio::spawn({
        let manager = manager.clone();
        let id = id.clone();
        async move { manager.acquire(&id).await }
    });
    store.entered.notified().await;
    let original = store.owner(&id).await.unwrap().unwrap();
    let draining = manager.begin_drain();
    tokio::pin!(draining);
    assert!(futures_util::poll!(&mut draining).is_pending());
    assert_eq!(manager.uncertain_acquisitions(), 1);
    store.resume.notify_one();
    assert!(matches!(
        acquiring.await.unwrap(),
        Err(OwnershipError::Stale)
    ));
    let owned = draining.await;
    assert_eq!(owned.len(), 1);
    assert_eq!(owned[0].token, original.token);
    assert!(!owned[0].is_current());
    assert_eq!(manager.uncertain_acquisitions(), 0);
    assert!(manager
        .handoff(&owned[0], async {
            Err(OwnershipError::Unavailable(
                "receiver acknowledgement absent".into(),
            ))
        })
        .await
        .is_err());
    assert_eq!(store.owner(&id).await.unwrap(), Some(original));
    manager
        .handoff(&owned[0], owned[0].revoke_transports())
        .await
        .unwrap();
    assert!(store.owner(&id).await.unwrap().is_none());
    let successor =
        OwnershipCoordinator::register(Arc::new(store.inner.clone()), member("successor"))
            .await
            .unwrap();
    let next = successor.acquire(&id).await.unwrap().unwrap();
    assert!(next.token.epoch > owned[0].token.epoch);
    store.release(&owned[0].token).await.unwrap();
    assert_eq!(store.owner(&id).await.unwrap().unwrap().token, next.token);
    store.inner.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn cancelled_committed_reply_is_retained_until_real_expiry_allows_one_successor() {
    let store = Held::new().await;
    let manager = OwnershipCoordinator::register(store.clone(), member("manager"))
        .await
        .unwrap();
    let id = AgentId("uncertain-acquisition".into());
    store.operation.store(3, Ordering::Release);
    let acquiring = tokio::spawn({
        let manager = manager.clone();
        let id = id.clone();
        async move { manager.acquire(&id).await }
    });
    store.entered.notified().await;
    let original = store.owner(&id).await.unwrap().unwrap();
    acquiring.abort();
    assert!(matches!(acquiring.await, Err(error) if error.is_cancelled()));
    assert!(manager.begin_drain().await.is_empty());
    assert_eq!(manager.uncertain_acquisitions(), 1);
    assert_eq!(store.owner(&id).await.unwrap(), Some(original.clone()));
    // The original lease, not an edited timestamp or receiver 404, determines
    // when a fresh process can become the next writer.
    tokio::time::sleep(std::time::Duration::from_millis(DEFAULT_LEASE_MS + 100)).await;
    assert!(store.owner(&id).await.unwrap().is_none());
    let successor =
        OwnershipCoordinator::register(Arc::new(store.inner.clone()), member("successor"))
            .await
            .unwrap();
    let next = successor.acquire(&id).await.unwrap().unwrap();
    assert_eq!(next.token.epoch, original.token.epoch + 1);
    assert!(store.check(&original.token).await.is_err());
    assert!(manager.acquire(&id).await.is_err());
    store.inner.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn real_member_heartbeat_and_owner_renewal_are_joined_before_durable_drain() {
    for operation in [2, 4] {
        let store = Held::new().await;
        let manager = OwnershipCoordinator::register(store.clone(), member("manager"))
            .await
            .unwrap();
        let id = AgentId("renewal-during-drain".into());
        let authority = manager.acquire(&id).await.unwrap().unwrap();
        store.operation.store(operation, Ordering::Release);
        let renewing = tokio::spawn({
            let manager = manager.clone();
            async move { manager.maintain().await }
        });
        store.entered.notified().await;
        let draining = manager.begin_drain();
        tokio::pin!(draining);
        assert!(futures_util::poll!(&mut draining).is_pending());
        assert!(!authority.is_current());
        store.resume.notify_one();
        assert!(renewing.await.unwrap().unwrap().is_empty());
        assert_eq!(draining.await.len(), 1);
        assert!(!*manager.failure_signal().borrow());
        assert!(store.membership().await.unwrap().members[0].draining);
        assert!(manager.maintain().await.unwrap().is_empty());
        manager
            .handoff(&authority, authority.revoke_transports())
            .await
            .unwrap();
        assert!(store.owner(&id).await.unwrap().is_none());
        store.inner.database.drop().await.unwrap();
    }
}
