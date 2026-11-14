use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Membership, OwnershipLease, OwnershipStore, OwnershipToken,
    DEFAULT_MAX_OWNED_AGENTS,
};
use std::sync::atomic::AtomicUsize;

struct DelayedStore {
    inner: Arc<MemoryOwnershipStore>,
    delay: Duration,
    calls: Mutex<BTreeMap<String, Vec<Instant>>>,
}

impl DelayedStore {
    fn new(delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(MemoryOwnershipStore::default()),
            delay,
            calls: Mutex::default(),
        })
    }

    fn record(&self, key: &str) {
        self.calls
            .lock()
            .unwrap()
            .entry(key.into())
            .or_default()
            .push(Instant::now());
    }
}

#[async_trait::async_trait]
impl OwnershipStore for DelayedStore {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
    }
    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.record("membership");
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
        self.inner.acquire(id, instance).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        tokio::time::sleep(self.delay).await;
        self.record(token.agent_id.as_str());
        self.inner.renew(token).await
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

async fn coordinator(store: Arc<DelayedStore>) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store,
        Member {
            instance_id: "manager".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://manager:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
        },
    )
    .await
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn stalled_housekeeping_does_not_delay_member_or_owner_renewals_at_capacity() {
    for count in [1, 10, 50, DEFAULT_MAX_OWNED_AGENTS] {
        let store = DelayedStore::new(Duration::from_millis(25));
        let coordinator = coordinator(store.clone()).await;
        let mut owners = Vec::new();
        for index in 0..count {
            owners.push(
                coordinator
                    .acquire(&AgentId(format!("agent-{index:03}")))
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        let work_calls = Arc::new(AtomicUsize::new(0));
        let work = work_calls.clone();
        let running = tokio::spawn(run(coordinator.clone(), move |_| {
            let calls = work.clone();
            async move {
                calls.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_secs(40)).await;
                Ok(())
            }
        }));
        // Tokio advances to each scheduled deadline, including delayed store IO.
        // This is a scheduling invariant, not a production latency measurement.
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert!(coordinator.is_ready());
        assert_eq!(work_calls.load(Ordering::Relaxed), 1);
        assert!(owners.iter().all(|owner| owner.is_current()));
        let max_gap = {
            let calls = store.calls.lock().unwrap();
            assert_eq!(calls.len(), count as usize + 1);
            let mut max_gap = Duration::ZERO;
            for samples in calls.values() {
                assert!(samples.len() >= 9);
                for pair in samples.windows(2) {
                    max_gap = max_gap.max(pair[1].duration_since(pair[0]));
                }
            }
            max_gap
        };
        assert!(max_gap <= Duration::from_millis(HEARTBEAT_INTERVAL_MS + 100));
        println!(
            "simulated_renewal count={count} max_gap_ms={}",
            max_gap.as_millis()
        );
        running.abort();
        assert!(running.await.unwrap_err().is_cancelled());
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_owner_store_fails_closed_without_waiting_for_housekeeping() {
    let store = DelayedStore::new(Duration::from_secs(10));
    let coordinator = coordinator(store.clone()).await;
    let owner = coordinator
        .acquire(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let started = Instant::now();
    let result = run(coordinator.clone(), |_| std::future::pending()).await;
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(6));
    assert!(!coordinator.is_ready());
    assert!(!owner.is_current());
    assert_eq!(
        store
            .inner
            .owner(&owner.token.agent_id)
            .await
            .unwrap()
            .unwrap()
            .token,
        owner.token
    );
}
