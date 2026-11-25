use super::*;
use layer3_adapter::owner_protocol::{
    ReceiverCleanup, ReceiverConfirmationCustody, ReceiverRevocation, ReceiverSettlementFailure,
    ReceiverSettlementWatch, TransportAuthority,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

pub(super) struct Store {
    pub inner: Arc<MongoAgentOwnership>,
    pub hold_acquire: AtomicBool,
    pub lose_acquire: AtomicBool,
    pub hold_renew: AtomicBool,
    pub lose_release: AtomicBool,
    pub entered: Notify,
    pub resume: Notify,
    pub releases: AtomicUsize,
}

impl Store {
    pub async fn new() -> Arc<Self> {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("use disposable Mongo");
        let client = Client::with_uri_str(uri).await.unwrap();
        let name = format!("cma_failure_containment_{}", uuid::Uuid::new_v4().simple());
        Arc::new(Self {
            inner: Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap()),
            hold_acquire: AtomicBool::new(false),
            lose_acquire: AtomicBool::new(false),
            hold_renew: AtomicBool::new(false),
            lose_release: AtomicBool::new(false),
            entered: Notify::new(),
            resume: Notify::new(),
            releases: AtomicUsize::new(0),
        })
    }

    pub async fn expired(&self, token: &OwnershipToken) {
        self.inner
            .owners
            .update_one(
                doc! {"_id": token.agent_id.as_str()},
                doc! {"$set": {"expires_at": mongodb::bson::DateTime::from_millis(0)}},
            )
            .await
            .unwrap();
    }
}

#[async_trait::async_trait]
impl OwnershipStore for Store {
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
        let result = self.inner.acquire(id, instance).await;
        if self.hold_acquire.load(Ordering::Acquire) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        if self.lose_acquire.swap(false, Ordering::AcqRel) && matches!(result, Ok(Some(_))) {
            return Err(OwnershipError::Unavailable(
                "synthetic lost acquisition reply".into(),
            ));
        }
        result
    }
    async fn reconcile_acquisition(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipToken>, OwnershipError> {
        self.inner.reconcile_acquisition(id, instance).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        if self.hold_renew.load(Ordering::Acquire) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        self.inner.renew(token).await
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.releases.fetch_add(1, Ordering::AcqRel);
        self.inner.release(token).await?;
        if self.lose_release.load(Ordering::Acquire) {
            Err(OwnershipError::Unavailable(
                "synthetic lost release reply".into(),
            ))
        } else {
            Ok(())
        }
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

pub(super) struct Receiver {
    pub acknowledges: AtomicBool,
    pub calls: AtomicUsize,
}

impl Receiver {
    pub fn install(authority: &Authority) -> Arc<Self> {
        let receiver = Arc::new(Self {
            acknowledges: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        authority.register_receiver(receiver.clone()).unwrap();
        authority
            .begin_receiver_confirmation(&receiver.identity())
            .unwrap();
        authority.receiver_confirmed(&receiver.identity());
        receiver
    }
}

#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "containment-policy-receiver".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        if self.acknowledges.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(layer3_adapter::AdapterError::Unavailable(
                "synthetic unavailable receiver".into(),
            ))
        }
    }
    async fn settle_typed(
        &self,
        _: ReceiverConfirmationCustody,
        _: &ReceiverSettlementWatch,
    ) -> Result<ReceiverCleanup, ReceiverSettlementFailure> {
        self.revoke()
            .await
            .map(|()| ReceiverCleanup::Acknowledged)
            .map_err(|_| ReceiverSettlementFailure::Unavailable)
    }
}

pub(super) async fn manager(
    store: Arc<dyn OwnershipStore>,
    name: &str,
    pacing: bool,
    capacity: u32,
) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register_with_policy(
        store,
        Member {
            instance_id: name.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://manager.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: capacity,
        },
        pacing,
        MaintenancePolicy::ContainFailures,
    )
    .await
    .unwrap()
}

pub(super) fn assigned(membership: &Membership, name: &str, prefix: &str) -> AgentId {
    (0..100)
        .map(|index| AgentId(format!("{prefix}-{index}")))
        .find(|id| {
            layer2_core::agent_ownership::candidates(id, membership, None).unwrap()[0].instance_id
                == name
        })
        .unwrap()
}

pub(super) async fn close(store: &Store, managers: &[Arc<OwnershipCoordinator>]) {
    for manager in managers {
        manager.stop_admission();
        let until = Instant::now() + Duration::from_secs(3);
        let _ = manager.prepare_drain(until).await;
        for authority in manager.owned().await {
            let _ = manager.drain_authority_until(&authority, until).await;
        }
        manager.abort_operations_until(until).await.unwrap();
    }
    let name = store.inner.database.name().to_owned();
    store.inner.database.drop().await.unwrap();
    assert!(!store
        .inner
        .client
        .list_database_names()
        .await
        .unwrap()
        .contains(&name));
}
