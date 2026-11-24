use super::*;
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

#[derive(Default)]
pub(super) struct Store {
    pub inner: test_support::MemoryOwnershipStore,
    pub release_calls: AtomicUsize,
    pub owner_calls: AtomicUsize,
    pub release_mode: AtomicUsize,
    pub observation: Mutex<Option<Option<OwnershipLease>>>,
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
        self.owner_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(observation) = self.observation.lock().unwrap().clone() {
            return Ok(observation);
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
        self.inner.renew(token).await
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.release_calls.fetch_add(1, Ordering::SeqCst);
        let mode = self.release_mode.load(Ordering::SeqCst);
        if mode != 1 {
            self.inner.release(token).await?;
        }
        if mode != 0 {
            return Err(OwnershipError::Unavailable(
                "synthetic release reply lost".into(),
            ));
        }
        Ok(())
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

pub(super) struct Receiver {
    identity: String,
    pub fail: AtomicBool,
    pub calls: AtomicUsize,
}

impl Receiver {
    pub fn install(authority: &Authority, identity: &str, fail: bool) -> Arc<Self> {
        let receiver = Arc::new(Self {
            identity: identity.into(),
            fail: AtomicBool::new(fail),
            calls: AtomicUsize::new(0),
        });
        authority.register_receiver(receiver.clone()).unwrap();
        authority.begin_receiver_confirmation(identity).unwrap();
        authority.receiver_confirmed(identity);
        receiver
    }
}

#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        self.identity.clone()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            Err(layer3_adapter::AdapterError::Unavailable(
                "synthetic receiver unavailable".into(),
            ))
        } else {
            Ok(())
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

pub(super) async fn manager(
    store: Arc<dyn OwnershipStore>,
    pacing: bool,
    capacity: u32,
) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register_with_policy(
        store,
        Member {
            instance_id: "manager".into(),
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
