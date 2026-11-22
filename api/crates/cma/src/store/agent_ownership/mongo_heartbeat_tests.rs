//! A transient member heartbeat failure against real Mongo skips one tick. It
//! extends no lease, and a peer still waits for durable owner expiry.
use super::*;
use layer2_core::agent_ownership::{Authority, OwnershipCoordinator, HEARTBEAT_INTERVAL_MS};
use mongodb::options::ClientOptions;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

// Only the faulted member's heartbeat uses the tagged client. Its renewals,
// effects and the peer's whole membership use an unaffected connection pool.
struct HeartbeatRouted {
    inner: Arc<MongoAgentOwnership>,
    heartbeat: MongoAgentOwnership,
}

#[async_trait::async_trait]
impl OwnershipStore for HeartbeatRouted {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
    }
    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        self.heartbeat.heartbeat(id, draining).await
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
        self.inner.renew(token).await
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

struct Fixture {
    store: Arc<MongoAgentOwnership>,
    app: String,
    faulted: Arc<OwnershipCoordinator>,
    peer: Arc<OwnershipCoordinator>,
    id: AgentId,
    owner: Arc<Authority>,
}

impl Fixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").expect("use disposable Mongo");
        let client = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_heartbeat_skip_{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
        store
            .database
            .create_collection("test_controls")
            .await
            .unwrap();
        let app = format!("heartbeat-skip-{}", uuid::Uuid::new_v4().simple());
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let tagged = Client::with_options(options).unwrap();
        let routed = Arc::new(HeartbeatRouted {
            inner: store.clone(),
            heartbeat: MongoAgentOwnership::open(&tagged, &name).await.unwrap(),
        });
        let faulted = OwnershipCoordinator::register(routed, member("heartbeat-faulted"))
            .await
            .unwrap();
        let peer = OwnershipCoordinator::register(store.clone(), member("heartbeat-peer"))
            .await
            .unwrap();
        let mut owned = None;
        for index in 0..128 {
            let id = AgentId(format!("heartbeat-agent-{index}"));
            if preferred(&store, &id).await == faulted.instance_id {
                let owner = faulted.acquire(&id).await.unwrap().unwrap();
                owned = Some((id, owner));
                break;
            }
        }
        let (id, owner) = owned.expect("fixture contains an Agent preferred by the faulted member");
        Self {
            store,
            app,
            faulted,
            peer,
            id,
            owner,
        }
    }

    async fn arm(&self, mode: mongodb::bson::Bson, fault: Document) {
        let mut data = doc! {"failCommands": ["update"], "appName": &self.app};
        data.extend(fault);
        self.store
            .client
            .database("admin")
            .run_command(doc! {"configureFailPoint": "failCommand", "mode": mode, "data": data})
            .await
            .unwrap();
    }

    async fn disarm(&self) {
        self.store
            .client
            .database("admin")
            .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
            .await
            .expect("disable injected fault before assertions");
    }

    async fn lease(&self) -> Option<OwnershipLease> {
        self.store.owner(&self.id).await.unwrap()
    }

    // A real fenced effect requires the live member row and exact owner token.
    async fn effect(&self, key: &str) -> Result<(), String> {
        let effect = async {
            let mut session = self.store.client.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            self.store
                .fence_transaction(&self.owner.token, &mut session)
                .await
                .map_err(|error| format!("fence refused: {error:?}"))?;
            self.store
                .database
                .collection::<Document>("test_controls")
                .insert_one(doc! {"_id": key})
                .session(&mut session)
                .await
                .unwrap();
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => Ok(()),
                _ => Err("effect commit was not confirmed".into()),
            }
        };
        self.owner
            .run(effect)
            .await
            .map_err(|_| "authority is not current".to_string())?
    }

    async fn close(&self) {
        self.disarm().await;
        self.faulted.begin_drain().await;
        self.peer.begin_drain().await;
        self.store.database.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo replica set"]
async fn slow_member_heartbeat_skips_one_tick_without_renewing_or_yielding_the_agent() {
    let fixture = Fixture::new().await;
    let original = fixture.lease().await.unwrap();
    // The blocked update still commits after the bounded call has ended.
    fixture
        .arm(
            doc! {"times": 1_i32}.into(),
            doc! {"blockConnection": true, "blockTimeMS": 2_500_i32},
        )
        .await;
    let started = Instant::now();
    let skipped = fixture.faulted.maintain().await;
    let elapsed = started.elapsed();
    let during_skip = fixture.lease().await;
    let peer_claim = fixture.peer.acquire(&fixture.id).await;
    let effect = fixture.effect("during-skipped-tick").await;
    let ready = fixture.faulted.is_ready();
    let failed = *fixture.faulted.failure_signal().borrow();
    let current = fixture.owner.is_current();
    tokio::time::sleep(Duration::from_secs(1)).await;
    fixture.disarm().await;
    let recovered = fixture.faulted.maintain().await;
    let renewed = fixture.lease().await;
    eprintln!(
        "heartbeat_skip elapsed_ms={} skipped={:?} ready={ready} failed={failed} current={current} effect={effect:?}",
        elapsed.as_millis(),
        skipped.as_ref().map(Vec::len),
    );
    fixture.close().await;

    assert!(skipped.unwrap().is_empty());
    assert!(elapsed >= Duration::from_millis(1_900));
    assert!(elapsed < Duration::from_secs(3), "heartbeat budget widened");
    assert!(ready && !failed && current);
    assert_eq!(during_skip, Some(original.clone()), "skipped tick renewed");
    assert!(peer_claim.unwrap().is_none(), "peer claimed a live owner");
    effect.unwrap();
    recovered.unwrap();
    let renewed = renewed.unwrap();
    assert_eq!(renewed.token, original.token);
    assert!(renewed.expires_at_ms > original.expires_at_ms);
}

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo replica set"]
async fn failing_member_heartbeats_end_at_membership_expiry_before_any_takeover() {
    let fixture = Fixture::new().await;
    fixture.faulted.maintain().await.unwrap();
    let original = fixture.lease().await.unwrap();
    fixture
        .arm("alwaysOn".into(), doc! {"errorCode": 91_i32})
        .await;
    let started = Instant::now();
    let tick = Duration::from_millis(HEARTBEAT_INTERVAL_MS);
    let mut next_tick = started + tick;
    let mut skipped = 0;
    let mut failed_at = None;
    let mut observations = Vec::new();
    let mut successor = None;
    while successor.is_none() && started.elapsed() < Duration::from_secs(40) {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if Instant::now() >= next_tick {
            next_tick += tick;
            fixture.peer.maintain().await.unwrap();
            match fixture.faulted.maintain().await {
                Ok(handoffs) if failed_at.is_none() && handoffs.is_empty() => skipped += 1,
                Ok(_) => observations.push("unexpected maintenance after failure"),
                Err(_) => {
                    failed_at.get_or_insert(started.elapsed());
                }
            }
        }
        // The durable lease may expire unrenewed; local authority must end first.
        match fixture.lease().await {
            Some(lease) if lease.token != original.token => {
                observations.push("durable owner changed before takeover");
            }
            None if fixture.owner.is_current() => {
                observations.push("local authority outlived its durable lease");
            }
            _ => (),
        }
        if let Some(next) = fixture.peer.acquire(&fixture.id).await.unwrap() {
            if fixture.owner.is_current() || failed_at.is_none() {
                observations.push("successor overlapped the faulted owner");
            }
            successor = Some(next);
        }
    }
    let old_check = fixture.store.check(&original.token).await;
    let late = fixture.effect("after-takeover").await;
    eprintln!(
        "heartbeat_outage skipped={skipped} failed_at_ms={:?} takeover_ms={} observations={observations:?}",
        failed_at.map(|at| at.as_millis()),
        started.elapsed().as_millis(),
    );
    fixture.close().await;

    assert!(observations.is_empty(), "{observations:?}");
    let successor = successor.expect("a healthy peer claims the Agent after expiry");
    assert!(successor.token.epoch > original.token.epoch);
    let failed_at = failed_at.expect("membership expiry stays process-fatal");
    assert!((3..=5).contains(&skipped), "skipped={skipped}");
    assert!(failed_at <= Duration::from_millis(DEFAULT_LEASE_MS + HEARTBEAT_INTERVAL_MS));
    assert_eq!(old_check, Err(OwnershipError::Stale));
    assert!(late.is_err(), "late effect used the expired owner");
}
