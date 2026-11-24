use std::sync::atomic::AtomicBool;

use tokio::sync::Notify;

use super::super::super::test_support::MemoryOwnershipStore;
use super::super::super::{Membership, OwnershipFailure, PROTOCOL_VERSION};
use super::*;

fn member(id: &str) -> Member {
    Member {
        instance_id: id.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: format!("http://{id}:8080"),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: 256,
    }
}

fn agent(id: &str) -> AgentId {
    AgentId(id.into())
}

async fn registered(store: &MemoryOwnershipStore, id: &str) -> bool {
    store
        .membership()
        .await
        .unwrap()
        .members
        .iter()
        .any(|member| member.instance_id == id)
}

#[tokio::test]
async fn a_prepared_member_is_invisible_and_acquires_nothing_until_activated() {
    for policy in [
        MaintenancePolicy::Legacy,
        MaintenancePolicy::ContainFailures,
    ] {
        let store = Arc::new(MemoryOwnershipStore::default());
        let starting = OwnershipCoordinator::prepare_with_policy(
            store.clone(),
            member("starting"),
            false,
            policy,
        );
        assert!(!starting.is_registered());
        assert!(!starting.is_ready());
        assert!(!registered(&store, "starting").await);
        assert!(matches!(
            starting.acquire(&agent("agent")).await,
            Err(OwnershipError::Stale)
        ));
        // Placement sees only the serving peer, so no invitation targets startup.
        let serving = OwnershipCoordinator::register(store.clone(), member("serving"))
            .await
            .unwrap();
        let membership = store.membership().await.unwrap();
        assert_eq!(
            candidates(&agent("agent"), &membership, None).unwrap()[0].instance_id,
            "serving"
        );
        assert!(serving.acquire(&agent("agent")).await.unwrap().is_some());

        starting.activate().await.unwrap();
        assert!(starting.is_registered());
        assert!(starting.is_ready());
        assert!(registered(&store, "starting").await);
        // Once registered it takes its share of placement like any member.
        let membership = store.membership().await.unwrap();
        let preferred = (0..64)
            .map(|index| agent(&format!("agent-{index}")))
            .find(|id| candidates(id, &membership, None).unwrap()[0].instance_id == "starting")
            .expect("some Agent prefers the new member");
        assert!(starting.acquire(&preferred).await.unwrap().is_some());
        // Activation is idempotent once registered.
        starting.activate().await.unwrap();
    }
}

#[tokio::test]
async fn admission_closed_before_activation_never_registers() {
    for policy in [
        MaintenancePolicy::Legacy,
        MaintenancePolicy::ContainFailures,
    ] {
        let store = Arc::new(MemoryOwnershipStore::default());
        let coordinator = OwnershipCoordinator::prepare_with_policy(
            store.clone(),
            member("terminating"),
            false,
            policy,
        );
        coordinator.stop_admission();
        assert!(matches!(
            coordinator.activate().await,
            Err(OwnershipError::Stale)
        ));
        assert!(!registered(&store, "terminating").await);
        assert!(coordinator.begin_drain().await.is_empty());
        assert!(!registered(&store, "terminating").await);
    }
}

#[tokio::test]
async fn an_unknown_registration_outcome_fails_the_process() {
    for policy in [
        MaintenancePolicy::Legacy,
        MaintenancePolicy::ContainFailures,
    ] {
        let store = Arc::new(MemoryOwnershipStore::default());
        let coordinator = OwnershipCoordinator::prepare_with_policy(
            store.clone(),
            member("partitioned"),
            false,
            policy,
        );
        store.partition(true);
        assert!(matches!(
            coordinator.activate().await,
            Err(OwnershipError::Unavailable(_))
        ));
        assert!(*coordinator.failure_signal().borrow());
        store.partition(false);
        assert!(coordinator.activate().await.is_err());
        assert!(!coordinator.is_ready());
    }
}

/// Holds one acquisition after its durable write has committed.
struct HeldAcquire {
    inner: Arc<MemoryOwnershipStore>,
    hold: AtomicBool,
    committed: Notify,
    resume: Notify,
}

#[async_trait::async_trait]
impl OwnershipStore for HeldAcquire {
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
    async fn has_owner_history(&self, id: &AgentId) -> Result<bool, OwnershipError> {
        self.inner.has_owner_history(id).await
    }
    async fn acquire(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        let acquired = self.inner.acquire(id, instance).await;
        if self.hold.swap(false, Ordering::AcqRel) {
            self.committed.notify_one();
            self.resume.notified().await;
        }
        acquired
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        self.inner.renew(token).await
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

#[tokio::test]
async fn drain_releases_an_acquisition_that_committed_while_admission_closed() {
    for policy in [
        MaintenancePolicy::Legacy,
        MaintenancePolicy::ContainFailures,
    ] {
        let inner = Arc::new(MemoryOwnershipStore::default());
        let store = Arc::new(HeldAcquire {
            inner: inner.clone(),
            hold: AtomicBool::new(true),
            committed: Notify::new(),
            resume: Notify::new(),
        });
        let coordinator = OwnershipCoordinator::register_with_policy(
            store.clone(),
            member("draining"),
            false,
            policy,
        )
        .await
        .unwrap();
        let id = agent("late");
        let acquiring = tokio::spawn({
            let coordinator = coordinator.clone();
            let id = id.clone();
            async move { coordinator.acquire(&id).await }
        });
        store.committed.notified().await;
        assert!(inner.owner(&id).await.unwrap().is_some());

        // Drain closes admission, then waits for the in-flight acquisition.
        let draining = tokio::spawn({
            let coordinator = coordinator.clone();
            async move { coordinator.begin_drain().await }
        });
        tokio::task::yield_now().await;
        assert!(coordinator.is_draining());
        store.resume.notify_one();
        let acquired = acquiring.await.unwrap();
        match policy {
            MaintenancePolicy::Legacy => assert!(matches!(acquired, Err(OwnershipError::Stale))),
            MaintenancePolicy::ContainFailures => assert!(acquired.unwrap().is_none()),
        }
        let owned = draining.await.unwrap();
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].token.agent_id, id);
        assert!(!owned[0].is_current());

        coordinator
            .handoff(&owned[0], async { Ok(()) })
            .await
            .unwrap();
        assert!(inner.owner(&id).await.unwrap().is_none());
    }
}
