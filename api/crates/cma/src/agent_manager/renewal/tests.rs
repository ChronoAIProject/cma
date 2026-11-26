use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Membership, OwnershipLease, OwnershipStore, OwnershipToken,
    DEFAULT_MAX_OWNED_AGENTS,
};
use std::sync::atomic::AtomicUsize;

#[path = "containment_tests.rs"]
mod containment;
#[path = "receiver_fairness_tests.rs"]
mod receiver_fairness;

struct DelayedStore {
    inner: Arc<MemoryOwnershipStore>,
    delay: Duration,
    heartbeat_delay_ms: AtomicUsize,
    unavailable_heartbeats: AtomicUsize,
    calls: Mutex<BTreeMap<String, Vec<Instant>>>,
}

impl DelayedStore {
    fn new(delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(MemoryOwnershipStore::default()),
            delay,
            heartbeat_delay_ms: AtomicUsize::new(20),
            unavailable_heartbeats: AtomicUsize::new(0),
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
        tokio::time::sleep(Duration::from_millis(
            self.heartbeat_delay_ms.load(Ordering::Relaxed) as u64,
        ))
        .await;
        self.record("membership");
        // Fail before the member row changes, as a refused store call would.
        if self
            .unavailable_heartbeats
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(OwnershipError::Unavailable("heartbeat refused".into()));
        }
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
        let delay = if token.agent_id.as_str() == "healthy" {
            Duration::from_millis(20)
        } else {
            self.delay
        };
        tokio::time::sleep(delay).await;
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
async fn stalled_owner_store_cancels_only_its_agent_without_waiting_for_housekeeping() {
    let store = DelayedStore::new(Duration::from_secs(10));
    let coordinator = coordinator(store.clone()).await;
    let owner = coordinator
        .acquire(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let started = Instant::now();
    let healthy = coordinator
        .acquire(&AgentId("healthy".into()))
        .await
        .unwrap()
        .unwrap();
    let running = tokio::spawn(run(coordinator.clone(), |_| std::future::pending()));
    tokio::time::timeout(Duration::from_secs(6), owner.revoked())
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(6));
    assert!(
        healthy.is_current(),
        "isolated renewal cancelled the supervised healthy peer"
    );
    assert!(coordinator.is_ready());
    assert!(!*coordinator.failure_signal().borrow());
    assert!(!owner.is_current());
    assert!(owner.receiver_cancelled());
    assert!(owner
        .run(async { panic!("cancelled owner executed") })
        .await
        .is_err());
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
    tokio::time::sleep(Duration::from_secs(20)).await;
    healthy
        .run(store.check(&healthy.token))
        .await
        .unwrap()
        .unwrap();
    assert!(!owner.is_current());
    assert!(!running.is_finished());
    coordinator.stop_admission();
    running.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn receiver_failure_retains_its_phase_and_original_error() {
    let store = DelayedStore::new(Duration::from_millis(25));
    let coordinator = coordinator(store).await;
    let owner = coordinator
        .acquire(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let failure = run(coordinator.clone(), |_| async {
        Err(OwnershipError::Unavailable(
            "receiver revocation was not acknowledged".into(),
        ))
    })
    .await
    .unwrap_err();
    assert_eq!(failure.phase, "receiver_housekeeping");
    assert_eq!(
        failure.error,
        OwnershipError::Unavailable("receiver revocation was not acknowledged".into())
    );
    assert!(coordinator.is_ready());
    let logs = finish_with_logs(&coordinator, Err(failure)).await;
    assert!(!coordinator.is_ready());
    assert!(!owner.is_current());
    assert!(*coordinator.failure_signal().borrow());
    assert!(logs.contains("phase=\"receiver_housekeeping\""), "{logs}");
    assert!(logs.contains("error_kind=\"unavailable\""), "{logs}");
    assert!(
        !logs.contains("receiver revocation was not acknowledged"),
        "{logs}"
    );
    assert!(logs.contains("during_drain=false"), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn transient_member_heartbeat_outage_keeps_owners_until_membership_ends() {
    // Ticks at 3, 6, 9 and 12 s precede the 15 s established membership.
    // Three skips are survived when the 12 s heartbeat succeeds; after a
    // fourth, the next tick falls at the lease end and always fails.
    for unavailable in [3, 4] {
        let store = DelayedStore::new(Duration::from_millis(25));
        store
            .unavailable_heartbeats
            .store(unavailable, Ordering::SeqCst);
        let coordinator = coordinator(store.clone()).await;
        let owner = coordinator
            .acquire(&AgentId("agent".into()))
            .await
            .unwrap()
            .unwrap();
        let started = Instant::now();
        let running = tokio::spawn(run(coordinator.clone(), |_| std::future::pending()));
        if unavailable == 3 {
            tokio::time::sleep(Duration::from_secs(30)).await;
            assert!(coordinator.is_ready());
            assert!(!*coordinator.failure_signal().borrow());
            assert!(owner.is_current());
            owner.run(store.check(&owner.token)).await.unwrap().unwrap();
            assert!(!running.is_finished());
            coordinator.stop_admission();
            running.await.unwrap().unwrap();
        } else {
            let failure = running.await.unwrap().unwrap_err();
            assert_eq!(started.elapsed(), Duration::from_secs(15));
            assert_eq!(failure.phase, "ownership_maintenance");
            assert_eq!(failure.error, OwnershipError::Stale);
            assert!(*coordinator.failure_signal().borrow());
            assert!(!coordinator.is_ready());
            assert!(!owner.is_current());
        }
        assert_eq!(store.unavailable_heartbeats.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn renewal_error_is_reported_after_its_failure_signal_started_drain() {
    let store = DelayedStore::new(Duration::from_millis(25));
    store.heartbeat_delay_ms.store(10_000, Ordering::Relaxed);
    let coordinator = coordinator(store).await;
    let owner = coordinator
        .acquire(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    // Each stalled heartbeat skips its tick; membership then ends on schedule.
    let started = Instant::now();
    let result = run(coordinator.clone(), |_| std::future::pending()).await;
    assert_eq!(started.elapsed(), Duration::from_secs(15));
    assert!(*coordinator.failure_signal().borrow());
    assert!(!owner.is_current());
    // Reproduce startup's failure observer stopping admission before spawn
    // handles the already-captured renewal result.
    coordinator.stop_admission();
    let logs = finish_with_logs(&coordinator, result).await;
    assert_eq!(logs.lines().count(), 1, "{logs}");
    assert!(logs.contains("phase=\"ownership_maintenance\""), "{logs}");
    assert!(logs.contains("error_kind=\"stale\""), "{logs}");
    assert!(
        !logs.contains("ownership operation exceeded its deadline"),
        "{logs}"
    );
    assert!(logs.contains("during_drain=true"), "{logs}");
    assert!(!coordinator.is_ready());
    assert!(!owner.is_current());
}

#[tokio::test(start_paused = true)]
async fn normal_drain_does_not_publish_an_ownership_failure() {
    let store = DelayedStore::new(Duration::from_millis(25));
    let coordinator = coordinator(store).await;
    coordinator.stop_admission();
    let result = run(coordinator.clone(), |_| std::future::pending()).await;
    assert!(result.is_ok());
    assert!(finish_with_logs(&coordinator, result).await.is_empty());
    assert!(!*coordinator.failure_signal().borrow());
}

#[tokio::test(start_paused = true)]
async fn receiver_cancellation_does_not_stop_healthy_supervised_owners() {
    use layer2_core::agent_ownership::candidates;
    use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};

    struct RefusedReceiver;
    #[async_trait::async_trait]
    impl ReceiverRevocation for RefusedReceiver {
        fn identity(&self) -> String {
            "receiver-without-revocation-ack".into()
        }

        async fn revoke(&self) -> layer3_adapter::Result<()> {
            Err(layer3_adapter::AdapterError::Unavailable(
                "receiver revocation acknowledgement unavailable".into(),
            ))
        }
    }

    let store = DelayedStore::new(Duration::from_millis(25));
    let manager = coordinator(store.clone()).await;
    let cancelled = manager
        .acquire(&AgentId("cancelled-receiver".into()))
        .await
        .unwrap()
        .unwrap();
    let healthy = manager
        .acquire(&AgentId("healthy-local".into()))
        .await
        .unwrap()
        .unwrap();
    let peer = OwnershipCoordinator::register(
        store.clone(),
        Member {
            instance_id: "peer".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://peer:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
        },
    )
    .await
    .unwrap();
    let membership = store.membership().await.unwrap();
    let peer_id = (0..128)
        .map(|index| AgentId(format!("healthy-peer-{index}")))
        .find(|id| {
            candidates(id, &membership, None)
                .unwrap()
                .first()
                .is_some_and(|member| member.instance_id == peer.instance_id)
        })
        .expect("fixture contains a peer-preferred Agent");
    let peer_owner = peer.acquire(&peer_id).await.unwrap().unwrap();
    cancelled
        .register_receiver(Arc::new(RefusedReceiver))
        .unwrap();
    let original = store
        .owner(&cancelled.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    // Model the receiver's irreversible cancellation before any renewal starts.
    // Adapter tests cover the negotiation errors that call this same method.
    TransportAuthority::revoke(cancelled.as_ref());
    let running = tokio::spawn(run(manager.clone(), |_| std::future::pending()));
    let peer_running = tokio::spawn(run(peer.clone(), |_| std::future::pending()));
    // Receiver cleanup is blocked. It cannot release or revive local authority,
    // and the supervisor must keep unrelated local/peer ownership available.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        manager.is_ready(),
        "receiver cancellation failed the whole manager"
    );
    assert!(peer.is_ready());
    assert!(!running.is_finished());
    assert!(!peer_running.is_finished());
    assert_eq!(
        store
            .owner(&cancelled.token.agent_id)
            .await
            .unwrap()
            .unwrap(),
        original,
        "no in-flight renewal and no receiver ACK: original durable lease remains"
    );
    assert!(!cancelled.is_current());
    let executed = AtomicUsize::new(0);
    assert!(cancelled
        .run(async { executed.fetch_add(1, Ordering::Relaxed) })
        .await
        .is_err());
    assert_eq!(executed.load(Ordering::Relaxed), 0);

    // Advance past a full lease lifetime, with both real supervisors running.
    tokio::time::sleep(Duration::from_secs(20)).await;
    for (coordinator, owner) in [(&manager, &healthy), (&peer, &peer_owner)] {
        assert!(coordinator.is_ready());
        assert!(owner.is_current());
        assert!(!*coordinator.failure_signal().borrow());
        owner.run(store.check(&owner.token)).await.unwrap().unwrap();
        assert_eq!(
            store
                .owner(&owner.token.agent_id)
                .await
                .unwrap()
                .unwrap()
                .token,
            owner.token
        );
    }
    assert!(!cancelled.is_current());
    manager.stop_admission();
    peer.stop_admission();
    running.await.unwrap().unwrap();
    peer_running.await.unwrap().unwrap();
}

#[derive(Clone, Default)]
struct LogOutput(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogOutput {
    type Writer = Self;

    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

async fn finish_with_logs(
    coordinator: &OwnershipCoordinator,
    result: Result<(), RenewalFailure>,
) -> String {
    let logs = LogOutput::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    finish(coordinator, result).await;
    let captured = logs.0.lock().unwrap().clone();
    String::from_utf8(captured).unwrap()
}
