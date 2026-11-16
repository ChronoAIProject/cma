//! Healthy maintenance may finish during deletion; real failures remain fatal.
use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;

#[derive(Default)]
struct Store {
    inner: test_support::MemoryOwnershipStore,
    gate: AtomicUsize, // 1: heartbeat, 2: owner renewal
    fault: AtomicUsize,
    entered: Notify,
    resume: Notify,
}

impl Store {
    async fn wait(&self, operation: usize) -> Result<(), OwnershipError> {
        if self.gate.load(Ordering::SeqCst) == operation {
            self.entered.notify_one();
            self.resume.notified().await;
            match self.fault.load(Ordering::SeqCst) {
                1 => {
                    return Err(OwnershipError::Unavailable(
                        "uncertain store outcome".into(),
                    ))
                }
                2 => return Err(OwnershipError::IncompatibleProtocol),
                3 => return Err(OwnershipError::Stale),
                _ => (),
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl OwnershipStore for Store {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
    }
    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        let mut result = self.inner.heartbeat(id, draining).await?;
        if !draining {
            self.wait(1).await?;
            let fault = self.fault.load(Ordering::SeqCst);
            match if self.gate.load(Ordering::SeqCst) == 1 || fault == 7 {
                fault
            } else {
                0
            } {
                4 => result.members.clear(),
                5 => result.members[0].protocol += 1,
                6 => result.members[0].expires_at_ms = result.now_ms,
                7 => result.members[0].expires_at_ms = result.now_ms + 1000,
                _ => (),
            }
        }
        Ok(result)
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
        self.inner.acquire(id, instance).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        let mut lease = self.inner.renew(token).await?;
        self.wait(2).await?;
        match self.fault.load(Ordering::SeqCst) {
            4 => lease.token.epoch += 1,
            5 => lease.token.protocol += 1,
            6 => lease.expires_at_ms = 0,
            _ => (),
        }
        Ok(lease)
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

async fn fixture() -> (Arc<Store>, Arc<OwnershipCoordinator>, Arc<Authority>) {
    let store = Arc::new(Store::default());
    let manager = OwnershipCoordinator::register(store.clone(), member("manager"))
        .await
        .unwrap();
    let authority = manager
        .acquire(&AgentId("owned".into()))
        .await
        .unwrap()
        .unwrap();
    (store, manager, authority)
}

async fn successful_overlap(operation: usize) {
    let (store, manager, authority) = fixture().await;
    store.gate.store(operation, Ordering::SeqCst);
    let running = tokio::spawn({
        let manager = manager.clone();
        async move { manager.maintain().await }
    });
    store.entered.notified().await;
    let original = store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    let retained = manager.begin_drain().await;
    assert_eq!(retained.len(), 1);
    assert!(!authority.is_current());
    assert!(authority.run(async {}).await.is_err());
    store.resume.notify_one();
    let result = running.await.unwrap();
    assert!(
        result.is_ok(),
        "healthy maintenance during planned drain must succeed: {:?}",
        result.as_ref().err()
    );
    assert!(!*manager.failure_signal().borrow());
    assert_eq!(
        store
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .unwrap(),
        original
    );
    let ack = Notify::new();
    let entered = Notify::new();
    let handoff = manager.handoff(&authority, async {
        entered.notify_one();
        ack.notified().await;
        Ok(())
    });
    tokio::pin!(handoff);
    tokio::select! {
        _ = entered.notified() => (),
        result = &mut handoff => panic!("handoff completed without ACK: {result:?}"),
    }
    assert_eq!(
        store
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .unwrap(),
        original
    );
    ack.notify_one();
    handoff.await.unwrap();
    assert!(store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(!*manager.failure_signal().borrow());
    assert!(!authority.is_current());
    assert!(manager.acquire(&authority.token.agent_id).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn healthy_heartbeat_finishing_after_planned_drain_is_not_process_failure() {
    successful_overlap(1).await;
}

#[tokio::test(start_paused = true)]
async fn healthy_owner_renewal_finishing_after_planned_drain_is_not_process_failure() {
    successful_overlap(2).await;
}

#[tokio::test(start_paused = true)]
async fn maintenance_entering_after_planned_drain_is_not_process_failure() {
    let (store, manager, authority) = fixture().await;
    manager.begin_drain().await;
    assert!(
        manager.maintain().await.is_ok(),
        "planned drain before maintenance entry must succeed"
    );
    assert!(!*manager.failure_signal().borrow());
    assert!(!authority.is_current());
    assert!(store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test(start_paused = true)]
async fn real_maintenance_errors_during_planned_drain_remain_fatal() {
    for operation in [1, 2] {
        for fault in 1..=6 {
            let (store, manager, authority) = fixture().await;
            store.gate.store(operation, Ordering::SeqCst);
            store.fault.store(fault, Ordering::SeqCst);
            let running = tokio::spawn({
                let manager = manager.clone();
                async move { manager.maintain().await }
            });
            store.entered.notified().await;
            manager.begin_drain().await;
            store.resume.notify_one();
            assert!(
                running.await.unwrap().is_err(),
                "operation {operation} fault {fault}"
            );
            assert!(*manager.failure_signal().borrow());
            assert!(!authority.is_current());
            assert!(store
                .owner(&authority.token.agent_id)
                .await
                .unwrap()
                .is_some());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn real_member_expiry_during_planned_drain_remains_fatal() {
    for operation in [1, 2] {
        let (store, manager, authority) = fixture().await;
        if operation == 1 {
            tokio::time::advance(Duration::from_millis(DEFAULT_LEASE_MS - 1000)).await;
        } else {
            store.fault.store(7, Ordering::SeqCst);
        }
        store.gate.store(operation, Ordering::SeqCst);
        let running = tokio::spawn({
            let manager = manager.clone();
            async move { manager.maintain().await }
        });
        store.entered.notified().await;
        manager.begin_drain().await;
        tokio::time::advance(Duration::from_millis(1001)).await;
        store.resume.notify_one();
        assert!(running.await.unwrap().is_err());
        assert!(*manager.failure_signal().borrow());
        assert!(!authority.is_current());
        assert!(manager
            .owned()
            .await
            .iter()
            .any(|held| held.token == authority.token));
    }
}
