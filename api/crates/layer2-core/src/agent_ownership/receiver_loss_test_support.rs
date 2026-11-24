//! Real coordinator/Authority cancellation coverage. The deterministic store
//! supplies valid leases; Mongo transaction and native HTTP tests are separate.
use super::super::test_support::MemoryOwnershipStore;
use super::super::*;
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::Notify;

pub(super) struct Receiver {
    pub(super) acknowledges: AtomicBool,
    pub(super) calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "receiver-loss-test".into()
    }

    async fn revoke(&self) -> layer3_adapter::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.acknowledges.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(layer3_adapter::AdapterError::Unavailable(
                "receiver has not acknowledged revocation".into(),
            ))
        }
    }

    async fn settle_typed(
        &self,
        _: layer3_adapter::owner_protocol::ReceiverConfirmationCustody,
        _: &layer3_adapter::owner_protocol::ReceiverSettlementWatch,
    ) -> Result<
        layer3_adapter::owner_protocol::ReceiverCleanup,
        layer3_adapter::owner_protocol::ReceiverSettlementFailure,
    > {
        self.revoke()
            .await
            .map(|()| layer3_adapter::owner_protocol::ReceiverCleanup::Acknowledged)
            .map_err(|_| layer3_adapter::owner_protocol::ReceiverSettlementFailure::Unavailable)
    }
}

pub(super) struct Store {
    pub(super) inner: Arc<MemoryOwnershipStore>,
    pub(super) pause_next: AtomicBool,
    pub(super) pause_failure: AtomicBool,
    pub(super) failure_entered: Notify,
    pub(super) failure_resume: Notify,
    pub(super) failure_writes: AtomicUsize,
    pub(super) renew_before_pause: bool,
    pub(super) entered: Notify,
    pub(super) resume: Notify,
    pub(super) cancelled_owner_renewals: AtomicUsize,
    pub(super) hide_renewal_until_publish: AtomicBool,
    pub(super) unpublished_renewal: AtomicBool,
}

#[async_trait::async_trait]
impl OwnershipStore for Store {
    fn receiver_diagnostics_enabled(&self) -> bool {
        self.inner.receiver_diagnostics_enabled()
    }

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
        if id.as_str() == "cancelled" && self.unpublished_renewal.load(Ordering::SeqCst) {
            // The race test queries only after the previous committed expiry.
            // A different snapshot cannot see the transaction awaiting commit.
            return Ok(None);
        }
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
        if token.agent_id.as_str() != "cancelled" {
            return self.inner.renew(token).await;
        }
        self.cancelled_owner_renewals.fetch_add(1, Ordering::SeqCst);
        if !self.pause_next.swap(false, Ordering::SeqCst) {
            return self.inner.renew(token).await;
        }
        let result = if self.renew_before_pause {
            Some(self.inner.renew(token).await)
        } else {
            None
        };
        if self.hide_renewal_until_publish.load(Ordering::SeqCst) {
            assert!(self.renew_before_pause);
            self.unpublished_renewal.store(true, Ordering::SeqCst);
        }
        self.entered.notify_one();
        self.resume.notified().await;
        self.unpublished_renewal.store(false, Ordering::SeqCst);
        match result {
            Some(result) => result,
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
        self.failure_writes.fetch_add(1, Ordering::SeqCst);
        if self.pause_failure.swap(false, Ordering::SeqCst) {
            self.failure_entered.notify_one();
            self.failure_resume.notified().await;
        }
        self.inner.record_receiver_failure(token, failure).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

async fn start(
    store: Arc<dyn OwnershipStore>,
    id: &str,
    policy: MaintenancePolicy,
) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register_with_policy(
        store,
        Member {
            instance_id: id.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: format!("http://{id}:8080"),
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

pub(super) struct Fixture {
    pub(super) store: Arc<Store>,
    pub(super) manager: Arc<OwnershipCoordinator>,
    pub(super) peer: Arc<OwnershipCoordinator>,
    pub(super) cancelled: Arc<Authority>,
    pub(super) healthy: Arc<Authority>,
    pub(super) peer_owner: Arc<Authority>,
    pub(super) receiver: Arc<Receiver>,
}

impl Fixture {
    pub(super) async fn new(renew_before_pause: bool) -> Self {
        Self::with_policy(renew_before_pause, MaintenancePolicy::Legacy).await
    }

    pub(super) async fn with_policy(renew_before_pause: bool, policy: MaintenancePolicy) -> Self {
        let inner = Arc::new(MemoryOwnershipStore::default());
        let store = Arc::new(Store {
            inner: inner.clone(),
            pause_next: AtomicBool::new(false),
            pause_failure: AtomicBool::new(false),
            failure_entered: Notify::new(),
            failure_resume: Notify::new(),
            failure_writes: AtomicUsize::new(0),
            renew_before_pause,
            entered: Notify::new(),
            resume: Notify::new(),
            cancelled_owner_renewals: AtomicUsize::new(0),
            hide_renewal_until_publish: AtomicBool::new(false),
            unpublished_renewal: AtomicBool::new(false),
        });
        let manager = start(store.clone(), "manager-a", policy).await;
        let cancelled = manager
            .acquire(&AgentId("cancelled".into()))
            .await
            .unwrap()
            .unwrap();
        let healthy = manager
            .acquire(&AgentId("healthy".into()))
            .await
            .unwrap()
            .unwrap();
        let peer = start(inner.client(), "manager-b", policy).await;
        let mut peer_owner = None;
        for index in 0..100 {
            if let Some(owner) = peer
                .acquire(&AgentId(format!("peer-{index}")))
                .await
                .unwrap()
            {
                peer_owner = Some(owner);
                break;
            }
        }
        let receiver = Arc::new(Receiver {
            acknowledges: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        cancelled.register_receiver(receiver.clone()).unwrap();
        Self {
            store,
            manager,
            peer,
            cancelled,
            healthy,
            peer_owner: peer_owner.unwrap(),
            receiver,
        }
    }

    pub(super) async fn healthy_work(&self) {
        assert!(self.manager.is_ready());
        assert!(!*self.manager.failure_signal().borrow());
        self.peer.maintain().await.unwrap();
        assert!(self.peer.is_ready());
        for authority in [&self.healthy, &self.peer_owner] {
            let token = authority.token.clone();
            assert_eq!(
                authority
                    .run(async {
                        self.store.inner.check(&token).await?;
                        Ok::<_, OwnershipError>(token.clone())
                    })
                    .await,
                Ok(Ok(token.clone()))
            );
            assert_eq!(
                self.store
                    .owner(&token.agent_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .token,
                token
            );
        }
    }

    pub(super) async fn refuse_then_acknowledge(&self, expiry: i64) {
        assert!(self
            .manager
            .handoff(&self.cancelled, self.cancelled.revoke_transports())
            .await
            .is_err());
        let retained = self
            .store
            .owner(&self.cancelled.token.agent_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.token, self.cancelled.token);
        assert_eq!(
            retained.expires_at_ms, expiry,
            "unacknowledged receiver must retain its actual durable lease"
        );
        self.manager.maintain().await.unwrap();
        let retry = self.manager.cancelled_receivers().await;
        assert!(
            retry
                .iter()
                .any(|owner| Arc::ptr_eq(owner, &self.cancelled)),
            "failed receiver cleanup must remain eligible for retry"
        );
        self.healthy_work().await;
        assert!(!self.cancelled.is_current());
        assert_eq!(
            self.store
                .owner(&self.cancelled.token.agent_id)
                .await
                .unwrap()
                .unwrap()
                .expires_at_ms,
            expiry
        );
        self.receiver.acknowledges.store(true, Ordering::SeqCst);
        self.manager
            .handoff(&self.cancelled, self.cancelled.revoke_transports())
            .await
            .unwrap();
        assert!(self.receiver.calls.load(Ordering::SeqCst) >= 2);
        assert!(self
            .store
            .owner(&self.cancelled.token.agent_id)
            .await
            .unwrap()
            .is_none());
        assert!(!self
            .manager
            .owned()
            .await
            .iter()
            .any(|owner| Arc::ptr_eq(owner, &self.cancelled)));
        assert_eq!(
            self.cancelled
                .renew_until(tokio::time::Instant::now() + Duration::from_secs(15)),
            Err(OwnershipError::Stale)
        );
        self.healthy_work().await;
    }
}
