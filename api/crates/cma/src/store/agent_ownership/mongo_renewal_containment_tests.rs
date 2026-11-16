//! Real Mongo WriteConflict exhaustion delivered by a still-running renewal.
use super::*;
use futures_util::FutureExt;
use layer2_core::agent_ownership::{Authority, OwnershipCoordinator, STORE_DEADLINE_MS};
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use std::panic::AssertUnwindSafe;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tokio::time::Instant;

type RenewalTask = JoinHandle<Result<OwnershipLease, OwnershipError>>;

#[derive(Default)]
struct Observation {
    elapsed: Duration,
    error: Option<OwnershipError>,
    blocker_aborted: bool,
}

struct ContendedStore {
    inner: MongoAgentOwnership,
    tokens: Mutex<Option<(OwnershipToken, OwnershipToken)>>,
    armed: AtomicBool,
    inflight: Mutex<Option<RenewalTask>>,
    unblocked: watch::Sender<bool>,
    observation: Arc<Mutex<Observation>>,
    actual_renewals: AtomicUsize,
    failed_deliveries: AtomicUsize,
    healthy_renewals: AtomicUsize,
}

impl ContendedStore {
    async fn join_fault(&self) {
        if let Some(mut task) = self.inflight.lock().await.take() {
            if tokio::time::timeout(Duration::from_secs(10), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
    }
}

#[async_trait::async_trait]
impl OwnershipStore for ContendedStore {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
    }
    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        let membership = self.inner.heartbeat(id, draining).await?;
        if id == "contained-member" && self.armed.swap(false, Ordering::SeqCst) {
            let (failed, healthy) = self.tokens.lock().await.clone().unwrap();
            let mut blocker = self.inner.client.start_session().await.unwrap();
            start_transaction(&mut blocker).await.unwrap();
            // Distinct owner rows deliberately contend on the SAME member row.
            self.inner
                .fence_transaction(&healthy, &mut blocker)
                .await
                .unwrap();
            let inner = self.inner.clone();
            let observation = self.observation.clone();
            let unblocked = self.unblocked.clone();
            self.actual_renewals.fetch_add(1, Ordering::SeqCst);
            let task = tokio::spawn(async move {
                let started = Instant::now();
                let result = inner.renew(&failed).await;
                let elapsed = started.elapsed();
                let blocker_aborted = blocker.abort_transaction().await.is_ok();
                *observation.lock().await = Observation {
                    elapsed,
                    error: result.as_ref().err().cloned(),
                    blocker_aborted,
                };
                unblocked.send_replace(true);
                result
            });
            *self.inflight.lock().await = Some(task);
            // Both production bounds are2s. Start the live store future earlier
            // so its actual error wins; never replace it with a saved/mock error.
            tokio::time::sleep(Duration::from_millis(600)).await;
            assert!(!self.inflight.lock().await.as_ref().unwrap().is_finished());
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
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        let tokens = self.tokens.lock().await.clone().unwrap();
        if *token == tokens.0 {
            self.failed_deliveries.fetch_add(1, Ordering::SeqCst);
            let mut slot = self.inflight.lock().await;
            let task = slot
                .as_mut()
                .expect("failed renewal must never be replayed");
            assert!(!task.is_finished(), "must consume a live store operation");
            // Retain the handle in the slot if coordinator's future is cancelled.
            let result = task.await.expect("Mongo fault task must not panic");
            slot.take();
            return result;
        }
        if *token == tokens.1 {
            self.unblocked
                .subscribe()
                .wait_for(|ready| *ready)
                .await
                .unwrap();
            self.healthy_renewals.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.renew(token).await
    }
}

struct Receiver(AtomicBool);
#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "mongo-containment-no-ack".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(layer3_adapter::AdapterError::Rejected(
                "fixture receiver has not acknowledged".into(),
            ))
        }
    }
}

async fn effect(store: &MongoAgentOwnership, authority: &Arc<Authority>) {
    authority
        .run(async {
            let mut session = store.client.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            store
                .fence_transaction(&authority.token, &mut session)
                .await
                .unwrap();
            assert!(matches!(
                commit_transaction(&mut session).await,
                CommitOutcome::Committed
            ));
        })
        .await
        .unwrap();
}

async fn exercise(store: Arc<ContendedStore>) {
    let manager = OwnershipCoordinator::register(store.clone(), member("contained-member"))
        .await
        .unwrap();
    let failed = manager
        .acquire(&AgentId("contained-agent".into()))
        .await
        .unwrap()
        .unwrap();
    let healthy = manager
        .acquire(&AgentId("healthy-local".into()))
        .await
        .unwrap()
        .unwrap();
    let peer = OwnershipCoordinator::register(store.clone(), member("peer-member"))
        .await
        .unwrap();
    let membership = store.membership().await.unwrap();
    let peer_id = (0..512)
        .map(|n| AgentId(format!("peer-agent-{n}")))
        .find(|id| {
            candidates(id, &membership, None)
                .unwrap()
                .first()
                .is_some_and(|m| m.instance_id == peer.instance_id)
        })
        .unwrap();
    let peer_owner = peer.acquire(&peer_id).await.unwrap().unwrap();
    let receiver = Arc::new(Receiver(AtomicBool::new(false)));
    failed.register_receiver(receiver.clone()).unwrap();
    let original = store.owner(&failed.token.agent_id).await.unwrap().unwrap();
    *store.tokens.lock().await = Some((failed.token.clone(), healthy.token.clone()));
    store.armed.store(true, Ordering::SeqCst);
    let (result, peer_result) = tokio::join!(manager.maintain(), peer.maintain());
    peer_result.unwrap();
    let observation = store.observation.lock().await;
    let raw_error = observation
        .error
        .as_ref()
        .expect("actual Mongo renewal error")
        .to_string();
    eprintln!(
        "m7_actual_mongo renewal_elapsed_ms={} error={raw_error}",
        observation.elapsed.as_millis()
    );
    assert!(raw_error.contains("deadline after a known abort"));
    assert!(raw_error.contains("WriteConflict") || raw_error.contains("TransientTransactionError"));
    assert!(observation.elapsed >= Duration::from_millis(STORE_DEADLINE_MS - 100));
    assert!(observation.elapsed < Duration::from_millis(STORE_DEADLINE_MS + 500));
    assert!(observation.blocker_aborted);
    drop(observation);
    assert!(
        result.is_ok(),
        "one Mongo renewal must not kill healthy membership: {:?}",
        result.as_ref().err()
    );
    assert!(manager.is_ready() && peer.is_ready());
    assert!(!*manager.failure_signal().borrow());
    assert!(!failed.is_current() && failed.receiver_cancelled());
    let executed = AtomicUsize::new(0);
    assert!(failed
        .run(async { executed.fetch_add(1, Ordering::SeqCst) })
        .await
        .is_err());
    assert_eq!(executed.load(Ordering::SeqCst), 0);
    effect(&store.inner, &healthy).await;
    effect(&store.inner, &peer_owner).await;
    assert_eq!(
        store.owner(&failed.token.agent_id).await.unwrap(),
        Some(original.clone())
    );
    assert!(manager
        .handoff(&failed, failed.revoke_transports())
        .await
        .is_err());
    assert_eq!(
        store.owner(&failed.token.agent_id).await.unwrap(),
        Some(original)
    );
    assert!(!manager.forget_cancelled(&failed).await.unwrap());
    for _ in 0..3 {
        manager.maintain().await.unwrap();
        peer.maintain().await.unwrap();
    }
    assert_eq!(store.actual_renewals.load(Ordering::SeqCst), 1);
    assert_eq!(store.failed_deliveries.load(Ordering::SeqCst), 1);
    assert_eq!(store.healthy_renewals.load(Ordering::SeqCst), 4);
    receiver.0.store(true, Ordering::SeqCst);
    manager
        .handoff(&failed, failed.revoke_transports())
        .await
        .unwrap();
    let count = |m: Membership| m.members.iter().map(|m| m.owned_agents).sum::<u32>();
    assert_eq!(count(store.membership().await.unwrap()), 2);
    let winner = preferred(&store.inner, &failed.token.agent_id).await;
    let next_manager = if winner == manager.instance_id {
        &manager
    } else {
        &peer
    };
    let successor = next_manager
        .acquire(&failed.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert!(successor.token.epoch > failed.token.epoch);
    manager
        .handoff(&failed, failed.revoke_transports())
        .await
        .unwrap();
    assert_eq!(
        store
            .owner(&failed.token.agent_id)
            .await
            .unwrap()
            .unwrap()
            .token,
        successor.token
    );
    assert_eq!(count(store.membership().await.unwrap()), 3);
    assert!(!failed.is_current());
    assert!(failed
        .renew_until(Instant::now() + Duration::from_secs(30))
        .is_err());
    effect(&store.inner, &successor).await;
}

#[tokio::test]
#[ignore = "requires private localhost Mongo replica set and cma_m7_ database"]
async fn actual_mongo_budget_exhaustion_contains_one_agent_and_preserves_peer_and_capacity() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("private Mongo URI required");
    let database =
        std::env::var("CHRONO_TEST_MONGODB_DATABASE").expect("scoped Mongo database required");
    assert!(
        database.starts_with("cma_m7_")
            && database
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
    );
    let options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
    assert!(!options.hosts.is_empty());
    assert!(options.hosts.iter().all(|address| matches!(address,
        mongodb::options::ServerAddress::Tcp { host, .. } if ["localhost", "127.0.0.1", "::1"].contains(&host.as_str()))));
    let client = Client::with_options(options).unwrap();
    let inner = MongoAgentOwnership::open(&client, &database).await.unwrap();
    let database = inner.database.clone();
    let store = Arc::new(ContendedStore {
        inner,
        tokens: Mutex::new(None),
        armed: AtomicBool::new(false),
        inflight: Mutex::new(None),
        unblocked: watch::channel(false).0,
        observation: Arc::new(Mutex::new(Observation::default())),
        actual_renewals: AtomicUsize::new(0),
        failed_deliveries: AtomicUsize::new(0),
        healthy_renewals: AtomicUsize::new(0),
    });
    let result = AssertUnwindSafe(exercise(store.clone()))
        .catch_unwind()
        .await;
    store.join_fault().await;
    let cleanup = database.drop().await;
    assert!(cleanup.is_ok(), "scoped Mongo database cleanup failed");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
