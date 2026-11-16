use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Notify;

#[derive(Default)]
pub struct Barrier {
    armed: AtomicBool,
    pub entered: Notify,
    pub resume: Notify,
}

impl Barrier {
    pub fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    async fn wait(&self) {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
    }
}

#[derive(Default)]
pub struct HeldStore {
    pub inner: Arc<test_support::MemoryOwnershipStore>,
    pub registration: Barrier,
    pub acquisition: Barrier,
    pub registrations: AtomicUsize,
    pub heartbeats: AtomicUsize,
    pub acquisitions: AtomicUsize,
    pub releases: AtomicUsize,
    pub fail_release: AtomicBool,
}

#[async_trait::async_trait]
impl OwnershipStore for HeldStore {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.registrations.fetch_add(1, Ordering::SeqCst);
        let result = self.inner.register(member).await;
        self.registration.wait().await;
        result
    }

    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        self.heartbeats.fetch_add(1, Ordering::SeqCst);
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
        self.acquisitions.fetch_add(1, Ordering::SeqCst);
        // Publication completes before the barrier; a cancelled response does
        // not roll back the durable token, matching an unknown store outcome.
        let result = self.inner.acquire(id, instance).await;
        self.acquisition.wait().await;
        result
    }

    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        self.inner.renew(token).await
    }

    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        if self.fail_release.load(Ordering::SeqCst) {
            return Err(OwnershipError::Unavailable(
                "release response unavailable".into(),
            ));
        }
        self.inner.release(token).await
    }

    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

pub fn member(instance: &str) -> Member {
    Member {
        instance_id: instance.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: format!("http://{instance}:8080"),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
    }
}

pub async fn assert_current(authority: &Arc<Authority>, store: &dyn OwnershipStore) {
    assert!(authority.is_current());
    store.check(&authority.token).await.unwrap();
    assert_eq!(
        authority
            .run(async { execution_owner().unwrap() })
            .await
            .unwrap(),
        authority.token
    );
}

pub async fn preferred_agent(store: &dyn OwnershipStore, instance: &str) -> AgentId {
    let membership = store.membership().await.unwrap();
    (0..1024)
        .map(|index| AgentId(format!("readiness-agent-{index}")))
        .find(|id| {
            candidates(id, &membership, None)
                .unwrap()
                .first()
                .is_some_and(|member| member.instance_id == instance)
        })
        .expect("a ready member has a placement candidate")
}
