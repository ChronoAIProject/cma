use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::Duration;
use tests::{key, persisted, request};

#[derive(Clone)]
struct ReadCommand {
    name: String,
    session: Option<Document>,
    concern: Option<Document>,
    transaction: bool,
}

struct Fixture {
    client: Client,
    database: String,
    source: Arc<MongoAgentAccessToolSource>,
    bearer: String,
    armed: Arc<AtomicBool>,
    commands: Arc<Mutex<Vec<ReadCommand>>>,
    gate: Arc<Mutex<Option<Arc<Gate>>>>,
}
struct Gate {
    entered: tokio::sync::Notify,
    released: Mutex<bool>,
    wake: std::sync::Condvar,
}

impl Gate {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
    fn wait(&self) {
        self.entered.notify_one();
        let (released, _) = self
            .wake
            .wait_timeout_while(
                self.released.lock().unwrap(),
                Duration::from_secs(10),
                |released| !*released,
            )
            .unwrap();
        assert!(*released, "test command gate was not released");
    }
}

impl Fixture {
    async fn new(delay: Duration) -> Self {
        let (client, database, _, state) = persisted().await;
        let armed = Arc::new(AtomicBool::new(false));
        let commands = Arc::new(Mutex::new(Vec::new()));
        let gate: Arc<Mutex<Option<Arc<Gate>>>> = Arc::new(Mutex::new(None));
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.app_name = Some(database.clone());
        options.command_event_handler = Some(EventHandler::callback({
            let armed = armed.clone();
            let commands = commands.clone();
            let gate = gate.clone();
            move |event| {
                let commands = commands.clone();
                let record = if armed.load(Ordering::SeqCst) {
                    if let CommandEvent::Started(event) = event {
                        Some(ReadCommand {
                            name: event.command_name,
                            session: event.command.get_document("lsid").ok().cloned(),
                            concern: event.command.get_document("readConcern").ok().cloned(),
                            transaction: event.command.contains_key("autocommit"),
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };
                let held = if record.as_ref().is_some_and(|r| r.name == "aggregate") {
                    gate.lock().unwrap().take()
                } else {
                    None
                };
                if let Some(record) = record {
                    commands.lock().unwrap().push(record);
                    if let Some(gate) = held {
                        gate.wait();
                    }
                    // The synchronous callback runs before command dispatch;
                    // an async callback is detached and cannot model latency.
                    std::thread::sleep(delay);
                }
            }
        }));
        let observed = Client::with_options(options).unwrap();
        let store = Arc::new(MongoSandboxStore::open(&observed, &database).await.unwrap());
        let source =
            MongoAgentAccessToolSource::new(store, "https://control.example.test", &key(9))
                .unwrap();
        let runtime = RuntimeHandle("access-runtime".into());
        let scope = validation::scope(&state, &runtime).ok().unwrap();
        let bearer = source.capability(&scope).ok().unwrap();
        source
            .resolve(&runtime, &request(&state), 1)
            .await
            .unwrap()
            .unwrap();
        Self {
            client,
            database,
            source,
            bearer,
            armed,
            commands,
            gate,
        }
    }
    fn hold_aggregate(&self) -> Arc<Gate> {
        let gate = Arc::new(Gate {
            entered: tokio::sync::Notify::new(),
            released: Mutex::new(false),
            wake: std::sync::Condvar::new(),
        });
        *self.gate.lock().unwrap() = Some(gate.clone());
        gate
    }
    fn start(&self) {
        self.commands.lock().unwrap().clear();
        self.armed.store(true, Ordering::SeqCst);
    }
    fn stop(&self) -> Vec<ReadCommand> {
        self.armed.store(false, Ordering::SeqCst);
        std::mem::take(&mut *self.commands.lock().unwrap())
    }
    async fn cleanup(self) {
        self.client.database(&self.database).drop().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn setup_snapshot_phase_measurement_and_command_contract() {
    let f = Fixture::new(Duration::from_millis(20)).await;
    for transaction in [true, false] {
        let mut samples = Vec::new();
        for _ in 0..20 {
            f.start();
            let started = tokio::time::Instant::now();
            if transaction {
                f.source.run(Operation::Setup(&f.bearer)).await.unwrap();
            } else {
                f.source.authenticate_setup(&f.bearer).await.unwrap();
            }
            samples.push(started.elapsed().as_micros() as u64);
            let commands = f.stop();
            let names: Vec<_> = commands.iter().map(|c| c.name.as_str()).collect();
            if transaction {
                assert_eq!(names, ["find", "aggregate", "commitTransaction"]);
            } else {
                assert_eq!(names, ["find", "aggregate"]);
                assert!(commands.iter().all(|c| !c.transaction));
                assert!(commands.iter().all(|c| c.session == commands[0].session));
                assert!(
                    commands
                        .iter()
                        .all(|c| c.concern.as_ref().unwrap().get_str("level").ok()
                            == Some("snapshot"))
                );
                assert!(commands[1]
                    .concern
                    .as_ref()
                    .unwrap()
                    .contains_key("atClusterTime"));
            }
        }
        samples.sort_unstable();
        assert!(
            samples[0] >= if transaction { 60_000 } else { 40_000 },
            "the injected command delay must be on the awaited path"
        );
        println!("setup_phase transaction={transaction} n=20 unit=us p50={} p95={} max={} commands_per_call={} samples={samples:?}", samples[9], samples[18], samples[19], if transaction {3} else {2});
    }
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn capability_and_large_lineage_fallback_share_one_snapshot_across_revocation() {
    for oversized in [false, true] {
        let f = Fixture::new(Duration::ZERO).await;
        if oversized {
            f.source
                .store
                .agents
                .update_one(
                    doc! {},
                    doc! {"$set":{"fixture_padding":"x".repeat(70 * 1024)}},
                )
                .await
                .unwrap();
        }
        f.start();
        let gate = f.hold_aggregate();
        let source = f.source.clone();
        let bearer = f.bearer.clone();
        let task = tokio::spawn(async move { source.authenticate_setup(&bearer).await });
        tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        // The first lookup already fixed atClusterTime. The lineage and any
        // fallback must observe that same point, just as the old transaction did.
        f.client
            .database(&f.database)
            .collection::<Document>("agents")
            .update_many(
                doc! {},
                doc! {"$set":{"record.requested_stop_reason":"owner"}},
            )
            .await
            .unwrap();
        f.client
            .database(&f.database)
            .collection::<Document>(COLLECTION)
            .delete_many(doc! {})
            .await
            .unwrap();
        gate.release();
        task.await.unwrap().unwrap();
        let commands = f.stop();
        assert!(commands.len() >= if oversized { 6 } else { 2 });
        let time = commands[1]
            .concern
            .as_ref()
            .unwrap()
            .get("atClusterTime")
            .unwrap();
        for command in &commands[1..] {
            assert!(!command.transaction);
            assert!(matches!(
                command.name.as_str(),
                "find" | "aggregate" | "getMore"
            ));
            assert_eq!(command.session, commands[0].session);
            if command.name != "getMore" {
                assert_eq!(
                    command.concern.as_ref().unwrap().get_str("level").ok(),
                    Some("snapshot")
                );
                assert_eq!(
                    command.concern.as_ref().unwrap().get("atClusterTime"),
                    Some(time)
                );
            }
        }
        f.start();
        assert!(matches!(
            f.source.authenticate_setup(&f.bearer).await,
            Err(AdapterError::Rejected(_))
        ));
        assert_eq!(
            f.stop().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["find"]
        );
        f.cleanup().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn cancelling_setup_does_not_commit_and_next_attempt_has_a_fresh_snapshot() {
    let f = Fixture::new(Duration::ZERO).await;
    f.start();
    let gate = f.hold_aggregate();
    let source = f.source.clone();
    let bearer = f.bearer.clone();
    let task = tokio::spawn(async move { source.authenticate_setup(&bearer).await });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    task.abort();
    gate.release();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(f
        .stop()
        .iter()
        .all(|c| !matches!(c.name.as_str(), "commitTransaction" | "abortTransaction")));
    f.start();
    f.source.authenticate_setup(&f.bearer).await.unwrap();
    let commands = f.stop();
    assert_eq!(commands.len(), 2);
    assert!(!commands[0]
        .concern
        .as_ref()
        .unwrap()
        .contains_key("atClusterTime"));
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn snapshot_failure_retries_are_bounded_and_cannot_turn_refusal_into_success() {
    let f = Fixture::new(Duration::ZERO).await;
    for (mode, code, expected_success) in [
        (doc! {"times":1}, 286, true),
        (doc! {"times":1}, 246, true),
        (doc! {"times":1}, 50, false),
    ] {
        f.client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":mode,
                "data":{"failCommands":["find"], "appName":&f.database, "errorCode":code}
            })
            .await
            .unwrap();
        f.start();
        let result = f.source.authenticate_setup(&f.bearer).await;
        f.client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert_eq!(result.is_ok(), expected_success);
        let commands = f.stop();
        assert_eq!(commands.len(), if expected_success { 3 } else { 1 });
        assert!(commands.iter().all(|c| !c.transaction));
    }
    f.client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":"alwaysOn",
            "data":{"failCommands":["find"], "appName":&f.database, "errorCode":286}
        })
        .await
        .unwrap();
    f.start();
    let result = f.source.authenticate_setup(&f.bearer).await;
    f.client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(AdapterError::Unavailable(_))));
    assert_eq!(f.stop().len(), TRANSACTION_ATTEMPTS);
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn two_hundred_setup_calls_keep_independent_snapshots_without_writes() {
    use futures_util::{stream, StreamExt, TryStreamExt};
    let f = Fixture::new(Duration::ZERO).await;
    f.start();
    stream::iter(0..200)
        .map(|_| f.source.authenticate_setup(&f.bearer))
        .buffer_unordered(20)
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let commands = f.stop();
    assert_eq!(commands.len(), 400);
    assert!(commands
        .iter()
        .all(|c| !c.transaction && matches!(c.name.as_str(), "find" | "aggregate")));
    f.cleanup().await;
}
