use super::*;
use crate::agent_ownership::{
    test_support::MemoryOwnershipStore, Membership, RenewalBatch, PROTOCOL_VERSION,
};
use std::sync::Mutex as StdMutex;
use tokio::sync::Notify;

#[derive(Clone)]
pub(super) enum Reply {
    Error(OwnershipError),
    Renew,
    DifferentToken(&'static str),
    ExpiredDeadline,
}

#[derive(Clone)]
pub(super) enum BatchReply {
    Renew,
    Partial,
    DifferentToken,
    Failure(OwnershipError),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RenewalEvent {
    Heartbeat,
    Chunk(Vec<AgentId>),
    Individual(AgentId),
}

pub(super) struct Store {
    inner: MemoryOwnershipStore,
    pub(super) reply: StdMutex<Reply>,
    pub(super) delay: StdMutex<Duration>,
    pub(super) all: AtomicBool,
    pub(super) heartbeat_mode: AtomicUsize,
    pub(super) heartbeat_starts: StdMutex<Vec<Instant>>,
    pub(super) calls: AtomicUsize,
    pub(super) releases: AtomicUsize,
    batch_supported: AtomicBool,
    pub(super) batch_reply: StdMutex<BatchReply>,
    pub(super) batch_delay: StdMutex<Duration>,
    pub(super) batch_events: StdMutex<Vec<RenewalEvent>>,
    active: AtomicUsize,
    pub(super) maximum: AtomicUsize,
    pub(super) gate: AtomicBool,
    pub(super) entered: Notify,
    pub(super) resume: Notify,
}

struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl OwnershipStore for Store {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
    }
    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        self.heartbeat_starts.lock().unwrap().push(Instant::now());
        if self.batch_supported.load(Ordering::SeqCst) {
            self.batch_events
                .lock()
                .unwrap()
                .push(RenewalEvent::Heartbeat);
        }
        // Modes 7-11 fail before the store row changes; 1 and 6 lose a committed reply.
        match self.heartbeat_mode.load(Ordering::SeqCst) {
            7 => {
                return Err(OwnershipError::Unavailable(
                    "heartbeat not committed".into(),
                ))
            }
            8 => return Err(OwnershipError::Stale),
            9 => return Err(OwnershipError::IncompatibleProtocol),
            10 => return Err(OwnershipError::Invalid("synthetic invalid member".into())),
            11 => {
                self.entered.notify_one();
                self.resume.notified().await;
                return Err(OwnershipError::Unavailable("gated heartbeat".into()));
            }
            _ => (),
        }
        let mut membership = self.inner.heartbeat(id, draining).await?;
        match self.heartbeat_mode.load(Ordering::SeqCst) {
            12 => {
                self.entered.notify_one();
                self.resume.notified().await;
            }
            1 => tokio::time::sleep(Duration::from_secs(10)).await,
            2 => membership.members[0].protocol += 1,
            3 => membership.members[0].draining = true,
            4 => membership.members.clear(),
            5 => membership.members[0].expires_at_ms = membership.now_ms + 500,
            6 => return Err(OwnershipError::Unavailable("heartbeat unavailable".into())),
            13 => membership.members[0].instance_id = "wrong-member".into(),
            14 => membership.members[0].expires_at_ms = membership.now_ms,
            15 => membership.members.push(membership.members[0].clone()),
            16 => membership.members[0].expires_at_ms = i64::MIN,
            _ => (),
        }
        Ok(membership)
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
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        if self.batch_supported.load(Ordering::SeqCst) {
            self.batch_events
                .lock()
                .unwrap()
                .push(RenewalEvent::Individual(token.agent_id.clone()));
        }
        if token.agent_id.as_str() != "agent-000" && !self.all.load(Ordering::SeqCst) {
            return self.inner.renew(token).await;
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        let _active = Active(&self.active);
        if self.gate.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        let delay = *self.delay.lock().unwrap();
        tokio::time::sleep(delay).await;
        let reply = self.reply.lock().unwrap().clone();
        match reply {
            Reply::Error(error) => Err(error),
            Reply::Renew => self.inner.renew(token).await,
            Reply::DifferentToken(field) => {
                let mut lease = self.inner.renew(token).await?;
                match field {
                    "agent" => lease.token.agent_id = AgentId("different-agent".into()),
                    "instance" => lease.token.instance_id = "different-instance".into(),
                    "epoch" => lease.token.epoch += 1,
                    "protocol" => lease.token.protocol += 1,
                    _ => panic!("unknown token field"),
                }
                Ok(lease)
            }
            Reply::ExpiredDeadline => {
                let mut lease = self.inner.renew(token).await?;
                lease.expires_at_ms = 0;
                Ok(lease)
            }
        }
    }

    fn supports_batch_renewal(&self) -> bool {
        self.batch_supported.load(Ordering::SeqCst)
    }

    async fn renew_batch(&self, tokens: &[OwnershipToken]) -> Result<RenewalBatch, OwnershipError> {
        assert!(self.batch_supported.load(Ordering::SeqCst));
        self.batch_events.lock().unwrap().push(RenewalEvent::Chunk(
            tokens.iter().map(|token| token.agent_id.clone()).collect(),
        ));
        if self.gate.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        let delay = *self.batch_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let reply = self.batch_reply.lock().unwrap().clone();
        match reply {
            BatchReply::Partial => Ok(RenewalBatch::PartialMatchAborted),
            BatchReply::Failure(error) => Err(error),
            BatchReply::Renew | BatchReply::DifferentToken => {
                let mut leases = Vec::new();
                for token in tokens {
                    leases.push(self.inner.renew(token).await?);
                }
                if matches!(reply, BatchReply::DifferentToken) {
                    leases.last_mut().unwrap().token.epoch += 1;
                }
                Ok(RenewalBatch::Committed(leases))
            }
        }
    }
}

pub(super) struct Fixture {
    pub(super) store: Arc<Store>,
    pub(super) manager: Arc<OwnershipCoordinator>,
    pub(super) owners: Vec<Arc<Authority>>,
}

impl Fixture {
    pub(super) async fn new(count: usize) -> Self {
        Self::with_policy(count, MaintenancePolicy::Legacy).await
    }

    pub(super) async fn with_batch(count: usize, supported: bool, enabled: bool) -> Self {
        let store = Self::store(supported);
        let manager = if enabled {
            let manager = OwnershipCoordinator::prepare_with_options(
                store.clone(),
                Self::member(),
                false,
                true,
            );
            manager.activate().await.unwrap();
            manager
        } else {
            OwnershipCoordinator::register_with_policy(
                store.clone(),
                Self::member(),
                false,
                MaintenancePolicy::Legacy,
            )
            .await
            .unwrap()
        };
        let mut owners = Vec::new();
        for index in 0..count {
            owners.push(
                manager
                    .acquire(&AgentId(format!("agent-{index:03}")))
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        Self {
            store,
            manager,
            owners,
        }
    }

    pub(super) async fn with_policy(count: usize, policy: MaintenancePolicy) -> Self {
        let store = Self::store(false);
        let manager = OwnershipCoordinator::register_with_policy(
            store.clone(),
            Self::member(),
            false,
            policy,
        )
        .await
        .unwrap();
        let mut owners = Vec::new();
        for index in 0..count {
            owners.push(
                manager
                    .acquire(&AgentId(format!("agent-{index:03}")))
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        Self {
            store,
            manager,
            owners,
        }
    }

    fn member() -> Member {
        Member {
            instance_id: "renewal-member".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://member.example.test:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        }
    }

    fn store(batch_supported: bool) -> Arc<Store> {
        Arc::new(Store {
            inner: MemoryOwnershipStore::default(),
            reply: StdMutex::new(Reply::Error(OwnershipError::Unavailable(
                "synthetic unavailable detail".into(),
            ))),
            delay: StdMutex::new(Duration::ZERO),
            all: AtomicBool::new(false),
            heartbeat_mode: AtomicUsize::new(0),
            heartbeat_starts: StdMutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            releases: AtomicUsize::new(0),
            batch_supported: AtomicBool::new(batch_supported),
            batch_reply: StdMutex::new(BatchReply::Renew),
            batch_delay: StdMutex::new(Duration::ZERO),
            batch_events: StdMutex::default(),
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
            gate: AtomicBool::new(false),
            entered: Notify::new(),
            resume: Notify::new(),
        })
    }

    pub(super) async fn leases(&self) -> Vec<OwnershipLease> {
        let mut leases = Vec::new();
        for authority in &self.owners {
            leases.push(
                self.store
                    .owner(&authority.token.agent_id)
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        leases
    }

    pub(super) fn assert_failed(&self) {
        assert!(!self.manager.is_ready());
        assert!(*self.manager.failure_signal().borrow());
        assert!(self.owners.iter().all(|authority| !authority.is_current()));
    }
}
