//! Force settlement overlap and uncertain commits through real Mongo replicas.
use super::*;
use layer2_core::budget::{BudgetPolicy, RequestHold};

struct Fixture {
    clock: Arc<AtomicU64>,
    admin: Client,
    name: String,
    app: String,
    store: Arc<MongoSandboxStore>,
    peer: Arc<MongoSandboxStore>,
    scope: RequestScope,
    request: ModelRequest,
    hold: RequestHold,
}

impl Fixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("cma_settlement_{suffix}");
        let app = format!("cma-settlement-{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let client = Client::with_options(options).unwrap();
        let (store, scope, request) = completed(&admin, &name).await;
        // Freeze requests at noon in the fixture's recorded UTC budget day.
        // Every independent client and reopened store shares this explicit clock;
        // wall-clock midnight during a failpoint cannot change the test oracle.
        let owner = store
            .model_budget
            .find_one(doc! {"_id": "owner:owner"})
            .await
            .unwrap()
            .unwrap();
        let day = owner
            .get_document("record")
            .unwrap()
            .get_i64("day")
            .unwrap() as u64;
        let clock = Arc::new(AtomicU64::new(day * 86_400 + 43_200));
        let mut store = (*store).clone();
        store.request_budget_clock = Some(clock.clone());
        let store = Arc::new(store);
        let hold = store
            .reserve_request(&scope, &request)
            .await
            .unwrap()
            .unwrap();
        let mut peer = MongoSandboxStore::open(&client, &name).await.unwrap();
        peer.request_budget_clock = Some(clock.clone());
        let peer = Arc::new(peer);
        Self {
            clock,
            admin,
            name,
            app,
            store,
            peer,
            scope,
            request,
            hold,
        }
    }

    async fn rows(&self) -> Vec<Document> {
        self.store
            .model_budget
            .find(doc! {})
            .sort(doc! {"_id": 1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap()
    }

    async fn reopen(&self) -> MongoSandboxStore {
        let mut store = MongoSandboxStore::open(&self.admin, &self.name)
            .await
            .unwrap();
        store.request_budget_clock = Some(self.clock.clone());
        store
    }

    async fn pause_settlement(&self) -> (i64, tokio::task::JoinHandle<Result<(), BudgetError>>) {
        let configured = self
            .admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
                "data": {"failCommands": ["update"], "appName": &self.app,
                    "blockConnection": true, "blockTimeMS": 5000_i32}
            })
            .await
            .unwrap();
        let count = configured
            .get_i64("count")
            .or_else(|_| configured.get_i32("count").map(i64::from))
            .unwrap();
        let peer = self.peer.clone();
        let hold = self.hold.clone();
        let pending = tokio::spawn(async move { peer.settle_request(&hold, usage()).await });
        (count, pending)
    }

    async fn reached(&self, count: i64) {
        self.admin
            .database("admin")
            .run_command(doc! {
                "waitForFailPoint": "failCommand", "timesEntered": count + 1,
                "maxTimeMS": 5000_i32,
            })
            .await
            .unwrap();
    }

    async fn disable(&self) {
        self.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
            .await
            .unwrap();
    }

    async fn cleanup(self) {
        self.disable().await;
        self.admin.database(&self.name).drop().await.unwrap();
    }
}

fn usage() -> RequestUsage {
    RequestUsage {
        input_tokens: 7,
        output_tokens: 1,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn controlled_day_rollover_retains_other_holds_and_refuses_backward_time() {
    let f = Fixture::new().await;
    let mut next = f.scope.clone();
    next.request_id = "retained-across-day".into();
    let mut small = f.request.clone();
    small.input_bytes = 1;
    let second = f
        .store
        .reserve_request(&next, &small)
        .await
        .unwrap()
        .unwrap();
    let before = f.rows().await;
    let initial = f.clock.load(Ordering::SeqCst);
    f.clock.store(initial - 86_400, Ordering::SeqCst);
    assert_eq!(
        f.peer.settle_request(&f.hold, usage()).await,
        Err(BudgetError::Unavailable)
    );
    next.request_id = "backward-day-refused".into();
    assert_eq!(
        f.peer.reserve_request(&next, &small).await,
        Err(BudgetError::Unavailable)
    );
    assert_eq!(f.rows().await, before);

    f.clock.store(initial + 86_400, Ordering::SeqCst);
    f.peer.settle_request(&f.hold, usage()).await.unwrap();
    let mut rolled = before;
    let owner = rolled
        .iter_mut()
        .find(|row| row.get_str("_id").unwrap() == "owner:owner")
        .unwrap();
    let record = owner.get_document_mut("record").unwrap();
    record.insert("day", record.get_i64("day").unwrap() + 1);
    record.insert("spent", 0_i64);
    let expected = expected_settlement(rolled, &f.hold);
    assert_eq!(f.rows().await, expected);
    let reopened = f.reopen().await;
    reopened.settle_request(&f.hold, usage()).await.unwrap();
    assert_eq!(
        reopened.reserve_request(&second.scope, &small).await,
        Err(BudgetError::Unavailable)
    );
    assert_eq!(f.rows().await, expected);
    next.request_id = "new-day-new-intent".into();
    assert!(reopened
        .reserve_request(&next, &f.request)
        .await
        .unwrap()
        .is_some());
    f.cleanup().await;
}

// Compare the complete budget collection: only this hold's exact debit and
// settlement may change. Parent evidence, other holds and call counts survive.
fn expected_settlement(mut rows: Vec<Document>, hold: &RequestHold) -> Vec<Document> {
    let request_key = format!("request:{}:{}", hold.scope.agent_id, hold.scope.request_id);
    let request = rows
        .iter()
        .find(|row| row.get_str("_id").unwrap() == request_key)
        .unwrap()
        .get_document("record")
        .unwrap();
    let reserved = request.get_i64("reserved").unwrap();
    let owner_key = format!("owner:{}", request.get_str("owner").unwrap());
    let policy: BudgetPolicy =
        mongodb::bson::deserialize_from_document(request.get_document("policy").unwrap().clone())
            .unwrap();
    let actual = i64::try_from(
        policy
            .value_usage(usage().input_tokens, usage().output_tokens)
            .unwrap(),
    )
    .unwrap();
    let ledger_key = format!(
        "request-turn:{}:{}",
        hold.scope.agent_id, hold.scope.turn_sequence
    );
    for row in &mut rows {
        let key = row.get_str("_id").unwrap().to_owned();
        let record = row.get_document_mut("record").unwrap();
        let period = key.starts_with("platform:");
        if key == owner_key || key == ledger_key || period {
            let remaining = record.get_i64("reserved").unwrap() - reserved;
            let spent = record.get_i64("spent").unwrap() + actual;
            record.insert("reserved", remaining);
            record.insert("spent", spent);
            if period {
                let revision = record.get_i64("revision").unwrap() + 1;
                record.insert("revision", revision);
            }
        } else if key == request_key {
            record.insert(
                "settled",
                mongodb::bson::serialize_to_bson(&usage()).unwrap(),
            );
        }
    }
    rows
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn concurrent_duplicate_settlement_debits_once_and_conflicts_preserve_every_record() {
    let f = Fixture::new().await;
    let before = f.rows().await;
    let (count, pending) = f.pause_settlement().await;
    f.reached(count).await;
    // The paused transaction has already read its snapshot, but has not saved
    // the owner debit. A separate store must win without double settlement.
    let winner = f.store.settle_request(&f.hold, usage()).await;
    let overlapped = !pending.is_finished();
    f.disable().await;
    let replay = pending.await.unwrap();
    assert!(
        overlapped,
        "the competing settlement must finish during the pause"
    );
    winner.unwrap();
    replay.unwrap();
    let expected = expected_settlement(before, &f.hold);
    assert_eq!(f.rows().await, expected);
    let conflicting = RequestUsage {
        input_tokens: usage().input_tokens + 1,
        ..usage()
    };
    assert_eq!(
        f.peer.settle_request(&f.hold, conflicting).await,
        Err(BudgetError::Unavailable)
    );
    assert_eq!(
        f.peer.reserve_request(&f.scope, &f.request).await,
        Err(BudgetError::Unavailable)
    );
    assert_eq!(f.rows().await, expected);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn settlement_racing_admission_keeps_other_holds_and_never_exposes_pending_release() {
    let f = Fixture::new().await;
    let before = f.rows().await;
    let (count, pending) = f.pause_settlement().await;
    f.reached(count).await;
    let mut next = f.scope.clone();
    next.request_id = "concurrent-reservation".into();
    let refused = f.store.reserve_request(&next, &f.request).await;
    let after_refusal = f.rows().await;
    let mut small = f.request.clone();
    small.input_bytes = 1;
    let admitted = f.store.reserve_request(&next, &small).await;
    let with_other_hold = f.rows().await;
    let overlapped = !pending.is_finished();
    f.disable().await;
    let settled = pending.await.unwrap();
    assert!(
        overlapped,
        "admission must finish before the paused settlement"
    );
    assert_eq!(refused, Err(BudgetError::TurnExhausted));
    assert_eq!(after_refusal, before);
    let second_hold = admitted.unwrap().unwrap();
    settled.unwrap();
    assert_eq!(
        f.rows().await,
        expected_settlement(with_other_hold, &f.hold)
    );
    let second_key = format!("request:{}:{}", next.agent_id, next.request_id);
    let rows = f.rows().await;
    let retained = rows
        .iter()
        .find(|row| row.get_str("_id").unwrap() == second_key)
        .unwrap();
    assert_eq!(
        retained.get_document("record").unwrap().get("settled"),
        Some(&mongodb::bson::Bson::Null)
    );
    assert_eq!(
        f.peer.reserve_request(&second_hold.scope, &small).await,
        Err(BudgetError::Unavailable)
    );
    next.request_id = "after-confirmed-settlement".into();
    assert!(f
        .store
        .reserve_request(&next, &f.request)
        .await
        .unwrap()
        .is_some());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unknown_settlement_commit_replays_once_after_restart_without_another_send_permit() {
    let f = Fixture::new().await;
    let before = f.rows().await;
    f.admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 5_i32},
            "data": {"failCommands": ["commitTransaction"], "appName": &f.app,
                "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected settlement commit ambiguity"}}
        })
        .await
        .unwrap();
    let result = f.peer.settle_request(&f.hold, usage()).await;
    f.disable().await;
    assert_eq!(result, Err(BudgetError::Unavailable));
    // The injected write-concern error skips the commit acknowledgement. Fence
    // majority visibility before another client's snapshot inspects the ledger.
    f.admin
        .database(&f.name)
        .collection::<Document>("settlement_visibility")
        .insert_one(doc! {"_id": "after-uncertain-commit"})
        .write_concern(mongodb::options::WriteConcern::majority())
        .await
        .unwrap();
    let expected = expected_settlement(before, &f.hold);
    assert_eq!(f.rows().await, expected);
    let reopened = f.reopen().await;
    reopened.settle_request(&f.hold, usage()).await.unwrap();
    assert_eq!(
        reopened.reserve_request(&f.scope, &f.request).await,
        Err(BudgetError::Unavailable)
    );
    assert_eq!(f.rows().await, expected);
    let mut next = f.scope.clone();
    next.request_id = "new-intent-after-settlement".into();
    assert!(reopened
        .reserve_request(&next, &f.request)
        .await
        .unwrap()
        .is_some());
    f.cleanup().await;
}
