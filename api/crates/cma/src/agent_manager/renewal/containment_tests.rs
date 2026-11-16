//! Faults at the ownership-store boundary run through the real CMA supervisor.
use super::*;
use layer2_core::agent_ownership::{DEFAULT_LEASE_MS, STORE_DEADLINE_MS};
use layer3_adapter::owner_protocol::TransportAuthority;

#[derive(Clone, Copy, Debug)]
enum RenewalFault {
    ExhaustedAbortBudget,
    UnknownCommit,
}

struct FaultStore {
    inner: MemoryOwnershipStore,
    fault: RenewalFault,
    failed_calls: AtomicUsize,
    healthy_calls: AtomicUsize,
    heartbeats: AtomicUsize,
}

#[async_trait::async_trait]
impl OwnershipStore for FaultStore {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
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
        self.inner.acquire(id, instance).await
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        if token.agent_id.as_str() != "failed" {
            // A healthy renewal must finish even when the other future errors first.
            tokio::time::sleep(Duration::from_millis(100)).await;
            self.healthy_calls.fetch_add(1, Ordering::SeqCst);
            return self.inner.renew(token).await;
        }
        self.failed_calls.fetch_add(1, Ordering::SeqCst);
        match self.fault {
            RenewalFault::ExhaustedAbortBudget => {
                // Inject the existing bounded store result, not a new coordinator retry.
                tokio::time::sleep(Duration::from_millis(STORE_DEADLINE_MS - 1)).await;
                Err(OwnershipError::Unavailable(
                    "Agent ownership transaction exceeded its deadline after a known abort: WriteConflict".into(),
                ))
            }
            RenewalFault::UnknownCommit => {
                self.inner.renew(token).await?;
                Err(OwnershipError::Unavailable(
                    "commit acknowledgement unknown".into(),
                ))
            }
        }
    }
}

async fn exercise_supervisor(fault: RenewalFault) {
    let store = Arc::new(FaultStore {
        inner: MemoryOwnershipStore::default(),
        fault,
        failed_calls: AtomicUsize::new(0),
        healthy_calls: AtomicUsize::new(0),
        heartbeats: AtomicUsize::new(0),
    });
    let manager = OwnershipCoordinator::register(
        store.clone(),
        Member {
            instance_id: "supervised-manager".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://fixture.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 2,
        },
    )
    .await
    .unwrap();
    let failed = manager
        .acquire(&AgentId("failed".into()))
        .await
        .unwrap()
        .unwrap();
    let healthy = manager
        .acquire(&AgentId("healthy".into()))
        .await
        .unwrap()
        .unwrap();
    let before = store.owner(&failed.token.agent_id).await.unwrap().unwrap();
    let (entered, entering) = tokio::sync::oneshot::channel();
    let pending_effect = tokio::spawn({
        let failed = failed.clone();
        async move {
            failed
                .run(async {
                    entered.send(()).unwrap();
                    std::future::pending::<()>().await
                })
                .await
        }
    });
    entering.await.unwrap();
    let receiver_cancelled = tokio::spawn({
        let failed = failed.clone();
        async move { TransportAuthority::revoked(failed.as_ref()).await }
    });
    // Slow cleanup cannot block membership or other owners, and must not be
    // mistaken for confirmed receiver revocation or a durable release.
    let running = tokio::spawn(run(manager.clone(), |_| std::future::pending()));
    tokio::time::sleep(Duration::from_millis(5_500)).await;
    assert!(!running.is_finished(), "{fault:?} ended the supervisor");
    assert!(manager.is_ready());
    assert!(!*manager.failure_signal().borrow());
    assert!(failed.receiver_cancelled());
    assert_eq!(pending_effect.await.unwrap(), Err(OwnershipError::Stale));
    receiver_cancelled.await.unwrap();
    let effects = AtomicUsize::new(0);
    assert!(failed
        .run(async { effects.fetch_add(1, Ordering::SeqCst) })
        .await
        .is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(manager.local(&failed.token.agent_id).await.is_none());
    let retained = store.owner(&failed.token.agent_id).await.unwrap().unwrap();
    assert_eq!(retained.token, before.token);
    match fault {
        RenewalFault::ExhaustedAbortBudget => assert_eq!(retained, before),
        RenewalFault::UnknownCommit => assert!(retained.expires_at_ms > before.expires_at_ms),
    }
    assert!(manager
        .acquire(&failed.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(!manager.forget_cancelled(&failed).await.unwrap());
    assert_eq!(store.membership().await.unwrap().members[0].owned_agents, 2);
    // Exercise repeated supervisor ticks beyond the original owner/member lease.
    tokio::time::sleep(Duration::from_millis(DEFAULT_LEASE_MS + 6_000)).await;
    assert!(!running.is_finished());
    assert!(manager.is_ready());
    assert!(healthy.is_current());
    healthy
        .run(store.check(&healthy.token))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.failed_calls.load(Ordering::SeqCst),
        1,
        "failed renewal was replayed"
    );
    assert!((7..=10).contains(&store.healthy_calls.load(Ordering::SeqCst)));
    assert!((7..=10).contains(&store.heartbeats.load(Ordering::SeqCst)));
    assert!(!failed.is_current());
    assert!(failed
        .renew_until(Instant::now() + Duration::from_secs(30))
        .is_err());
    assert!(manager
        .cancelled_receivers()
        .await
        .iter()
        .any(|owner| Arc::ptr_eq(owner, &failed)));
    manager.stop_admission();
    let result = running.await.unwrap();
    assert!(result.is_ok());
    assert!(finish_with_logs(&manager, result).await.is_empty());
    assert!(!*manager.failure_signal().borrow());
}

#[tokio::test(start_paused = true)]
async fn exhausted_owner_budget_preserves_supervised_member_peer_and_rejects_effects() {
    exercise_supervisor(RenewalFault::ExhaustedAbortBudget).await;
}

#[tokio::test(start_paused = true)]
async fn unknown_commit_is_not_replayed_by_later_supervisor_ticks() {
    exercise_supervisor(RenewalFault::UnknownCommit).await;
}

#[tokio::test(start_paused = true)]
async fn stalled_owner_store_cancels_only_that_owner_without_waiting_for_housekeeping() {
    let store = DelayedStore::new(Duration::from_secs(10));
    let manager = coordinator(store.clone()).await;
    let owner = manager
        .acquire(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let running = tokio::spawn(run(manager.clone(), |_| std::future::pending()));
    tokio::time::sleep(Duration::from_millis(5_500)).await;
    assert!(!running.is_finished());
    assert!(manager.is_ready());
    assert!(!*manager.failure_signal().borrow());
    assert!(!owner.is_current());
    assert!(owner.receiver_cancelled());
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
    manager.stop_admission();
    running.await.unwrap().unwrap();
}
