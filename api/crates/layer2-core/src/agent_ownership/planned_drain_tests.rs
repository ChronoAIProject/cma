//! Healthy maintenance may finish during deletion; real failures remain fatal.
use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;

#[derive(Default)]
struct Store {
    inner: test_support::MemoryOwnershipStore,
    registrations: AtomicUsize,
    gate: AtomicUsize, // 1: heartbeat, 2: owner renewal, 3: registration
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
        self.registrations.fetch_add(1, Ordering::SeqCst);
        let mut result = self.inner.register(member).await?;
        self.wait(3).await?;
        match self.fault.load(Ordering::SeqCst) {
            4 => result.members.clear(),
            5 => result.members[0].protocol += 1,
            6 => result.members[0].expires_at_ms = result.now_ms,
            _ => (),
        }
        Ok(result)
    }
    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        if draining && self.fault.load(Ordering::SeqCst) == 8 {
            return Err(OwnershipError::Unavailable(
                "drain advertisement unavailable".into(),
            ));
        }
        if !draining {
            self.wait(1).await?;
        }
        let mut result = self.inner.heartbeat(id, draining).await?;
        if !draining {
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
        self.wait(2).await?;
        let mut lease = self.inner.renew(token).await?;
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
    let drain = manager.begin_drain();
    tokio::pin!(drain);
    let joined = futures_util::poll!(&mut drain).is_pending();
    assert!(!authority.is_current());
    assert!(authority.run(async {}).await.is_err());
    store.resume.notify_one();
    let (result, retained) = tokio::join!(running, drain);
    let result = result.unwrap();
    assert!(
        joined,
        "planned drain must join in-flight maintenance before durable advertisement"
    );
    assert_eq!(retained.len(), 1);
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
            .unwrap()
            .token,
        original.token
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
            .unwrap()
            .token,
        original.token
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
            let drain = manager.begin_drain();
            tokio::pin!(drain);
            assert!(futures_util::poll!(&mut drain).is_pending());
            store.resume.notify_one();
            let (result, _) = tokio::join!(running, drain);
            assert!(
                result.unwrap().is_err(),
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
        let drain = manager.begin_drain();
        tokio::pin!(drain);
        assert!(futures_util::poll!(&mut drain).is_pending());
        tokio::time::advance(Duration::from_millis(1001)).await;
        store.resume.notify_one();
        let (result, _) = tokio::join!(running, drain);
        assert!(result.unwrap().is_err());
        assert!(*manager.failure_signal().borrow());
        assert!(!authority.is_current());
        assert!(manager
            .owned()
            .await
            .iter()
            .any(|held| held.token == authority.token));
    }
}

#[tokio::test(start_paused = true)]
async fn never_registered_planned_drain_does_not_signal_maintenance_failure() {
    let store = Arc::new(Store::default());
    let manager = OwnershipCoordinator::prepare(store.clone(), member("never-ready"));
    assert!(manager.begin_drain().await.is_empty());
    assert!(
        manager.maintain().await.is_ok(),
        "never-ready planned drain must not fail maintenance"
    );
    assert!(!*manager.failure_signal().borrow());
    assert!(!manager.can_acquire());
    assert!(manager.activate().await.is_err());
    assert!(manager.acquire(&AgentId("forbidden".into())).await.is_err());
    assert!(store.membership().await.unwrap().members.is_empty());
    manager.fail().await;
    assert!(manager.maintain().await.is_err());
    assert!(*manager.failure_signal().borrow());
}

#[tokio::test(start_paused = true)]
async fn failed_or_unknown_registration_during_planned_drain_remains_fatal() {
    for fault in [1, 2, 3, 4, 5, 6, 8] {
        let store = Arc::new(Store::default());
        store.gate.store(3, Ordering::SeqCst);
        store.fault.store(fault, Ordering::SeqCst);
        let manager = OwnershipCoordinator::prepare(store.clone(), member("starting"));
        let activating = tokio::spawn({
            let manager = manager.clone();
            async move { manager.activate().await }
        });
        store.entered.notified().await;
        manager.stop_admission();
        let draining = manager.begin_drain();
        tokio::pin!(draining);
        assert!(futures_util::poll!(&mut draining).is_pending());
        store.resume.notify_one();
        let (activated, owned) = tokio::join!(activating, draining);
        assert!(activated.unwrap().is_err(), "fault {fault}");
        assert!(*manager.failure_signal().borrow(), "fault {fault}");
        assert!(owned.is_empty());
        assert!(!manager.can_acquire());
        assert!(manager.activate().await.is_err());
        assert_eq!(store.registrations.load(Ordering::SeqCst), 1);
    }
}
