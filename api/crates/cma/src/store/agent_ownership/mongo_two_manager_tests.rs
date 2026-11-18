//! The peer continues fenced work while another manager retries a held member row.
use super::*;
use layer2_core::agent_ownership::OwnershipCoordinator;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::Notify;

// The real heartbeat must finish before the effect takes the member lock;
// otherwise a nontransactional heartbeat can simply wait out the contention.
// All ownership behavior is delegated to the actual Mongo store.
struct AfterHeartbeat {
    store: Arc<MongoAgentOwnership>,
    armed: AtomicBool,
    heartbeat_done: Notify,
    effect_held: Notify,
    peer_start: Notify,
    holding: AtomicBool,
}

#[async_trait::async_trait]
impl OwnershipStore for AfterHeartbeat {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.store.register(member).await
    }
    async fn heartbeat(
        &self,
        instance: &str,
        draining: bool,
    ) -> Result<Membership, OwnershipError> {
        let membership = self.store.heartbeat(instance, draining).await?;
        if instance == "manager-a" && self.armed.swap(false, Ordering::SeqCst) {
            self.heartbeat_done.notify_one();
            self.effect_held.notified().await;
        }
        Ok(membership)
    }
    async fn membership(&self) -> Result<Membership, OwnershipError> {
        self.store.membership().await
    }
    async fn owner(&self, id: &AgentId) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.store.owner(id).await
    }
    async fn has_owner_history(&self, id: &AgentId) -> Result<bool, OwnershipError> {
        self.store.has_owner_history(id).await
    }
    async fn acquire(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.store.acquire(id, instance).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        self.store.renew(token).await
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.store.release(token).await
    }
    async fn record_receiver_failure(
        &self,
        token: &OwnershipToken,
        failure: OwnershipFailure,
    ) -> Result<(), OwnershipError> {
        self.store.record_receiver_failure(token, failure).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.store.check(token).await
    }
}

async fn scoped_fixture() -> MongoAgentOwnership {
    let database = match std::env::var("CHRONO_TEST_MONGODB_DATABASE") {
        Ok(name) => name,
        Err(std::env::VarError::NotPresent) => return fixture().await,
        Err(error) => panic!("invalid disposable Mongo database: {error}"),
    };
    assert!(database.starts_with("cma_"));
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("use disposable Mongo");
    let client = Client::with_uri_str(uri).await.unwrap();
    MongoAgentOwnership::open(&client, &database).await.unwrap()
}

fn assigned_agent(membership: &Membership, prefix: &str, manager: &str) -> AgentId {
    (0..100)
        .map(|index| AgentId(format!("{prefix}-{index}")))
        .find(|id| candidates(id, membership, None).unwrap()[0].instance_id == manager)
        .expect("fixture has an Agent assigned to each actual manager")
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn two_managers_keep_serving_through_one_members_bounded_effect_contention() {
    let store = Arc::new(scoped_fixture().await);
    let gated = Arc::new(AfterHeartbeat {
        store: store.clone(),
        armed: AtomicBool::new(false),
        heartbeat_done: Notify::new(),
        effect_held: Notify::new(),
        peer_start: Notify::new(),
        holding: AtomicBool::new(false),
    });
    let a = OwnershipCoordinator::register(gated.clone(), member("manager-a"))
        .await
        .unwrap();
    let b = OwnershipCoordinator::register(store.clone(), member("manager-b"))
        .await
        .unwrap();
    let membership = store.membership().await.unwrap();
    let effect_owner = a
        .acquire(&assigned_agent(&membership, "effect", "manager-a"))
        .await
        .unwrap()
        .unwrap();
    let renewing_owner = a
        .acquire(&assigned_agent(&membership, "renew", "manager-a"))
        .await
        .unwrap()
        .unwrap();
    let peer_owner = b
        .acquire(&assigned_agent(&membership, "peer", "manager-b"))
        .await
        .unwrap()
        .unwrap();
    store
        .database
        .create_collection("test_fenced_effects")
        .await
        .unwrap();
    let effects = store.database.collection::<Document>("test_fenced_effects");
    gated.armed.store(true, Ordering::SeqCst);

    let (a_result, effect_commit, peer_result) = tokio::join!(
        a.maintain(),
        async {
            tokio::time::timeout(Duration::from_secs(2), gated.heartbeat_done.notified())
                .await
                .unwrap();
            let mut session = store.client.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            store
                .fence_transaction(&effect_owner.token, &mut session)
                .await
                .unwrap();
            gated.holding.store(true, Ordering::SeqCst);
            gated.effect_held.notify_one();
            gated.peer_start.notify_one();
            tokio::time::sleep(Duration::from_millis(400)).await;
            let outcome = commit_transaction(&mut session).await;
            gated.holding.store(false, Ordering::SeqCst);
            outcome
        },
        async {
            tokio::time::timeout(Duration::from_secs(2), gated.peer_start.notified())
                .await
                .map_err(unavailable)?;
            b.maintain().await?;
            peer_owner
                .run(async {
                    let mut session = store.client.start_session().await.map_err(unavailable)?;
                    start_transaction(&mut session).await.map_err(unavailable)?;
                    store
                        .fence_transaction(&peer_owner.token, &mut session)
                        .await
                        .map_err(unavailable)?;
                    effects
                        .insert_one(doc! {"_id": "peer-kept-serving"})
                        .session(&mut session)
                        .await
                        .map_err(unavailable)?;
                    match commit_transaction(&mut session).await {
                        CommitOutcome::Committed => Ok(()),
                        _ => Err(unavailable("peer fenced effect was not committed")),
                    }
                })
                .await??;
            Ok::<_, OwnershipError>(gated.holding.load(Ordering::SeqCst))
        }
    );

    let peer_effect = effects
        .find_one(doc! {"_id": "peer-kept-serving"})
        .await
        .unwrap();
    let mut stored_tokens = Vec::new();
    for authority in [&effect_owner, &renewing_owner, &peer_owner] {
        stored_tokens.push(
            store
                .owner(&authority.token.agent_id)
                .await
                .unwrap()
                .unwrap()
                .token,
        );
    }
    let live = store.membership().await.unwrap();
    store.database.drop().await.unwrap();
    eprintln!("two_manager_contention a_result={:?} a_failed={} a_ready={} b_ready={} peer_committed_while_held={peer_result:?}", a_result.as_ref().map(Vec::len), *a.failure_signal().borrow(), a.is_ready(), b.is_ready());
    // Prove the peer, fixture and durable identities before the expected RED.
    assert_eq!(
        peer_result,
        Ok(true),
        "peer must commit during A's held transaction"
    );
    assert!(peer_effect.is_some());
    assert!(b.is_ready() && peer_owner.is_current());
    assert!(matches!(effect_commit, CommitOutcome::Committed));
    assert_eq!(
        stored_tokens,
        vec![
            effect_owner.token.clone(),
            renewing_owner.token.clone(),
            peer_owner.token.clone()
        ]
    );
    for instance in ["manager-a", "manager-b"] {
        assert!(live
            .members
            .iter()
            .any(|member| member.instance_id == instance
                && !member.draining
                && member.protocol == PROTOCOL_VERSION
                && member.expires_at_ms > live.now_ms));
    }
    assert!(
        a_result.is_ok(),
        "two-manager bounded contention must not fail manager A: {:?}",
        a_result.as_ref().map(Vec::len)
    );
    assert!(a.is_ready() && effect_owner.is_current() && renewing_owner.is_current());
    assert!(!*a.failure_signal().borrow());
}
