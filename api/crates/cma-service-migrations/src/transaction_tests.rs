use super::*;
use crate::tests::{fixture, LEDGER};
use crate::Runner;
use mongodb::{bson::Document, options::ClientOptions, Client};
use std::{
    future::{Future, IntoFuture},
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
struct AbortLog(Arc<Mutex<Vec<u8>>>);

impl Write for AbortLog {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl AbortLog {
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let buffer = self.clone();
        tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || buffer.clone())
            .finish()
    }

    fn contains(&self, message: &str) -> bool {
        String::from_utf8_lossy(&self.0.lock().unwrap()).contains(message)
    }
}

#[tokio::test]
async fn lost_abort_task_is_reported_without_replacing_the_refusal() {
    let log = AbortLog::default();
    let task = tokio::spawn(std::future::pending::<()>());
    task.abort();
    let refusal = finish_abort(task, "activate", "original refusal")
        .with_subscriber(log.subscriber())
        .await;
    assert_eq!(refusal, "original refusal");
    assert!(log.contains("migration abort task did not finish"));
    assert!(log.contains("cancelled=true"));
}

#[test]
fn uncertain_abort_replies_are_not_acknowledged() {
    assert!(acknowledged_abort(&doc! {"ok": 1.0}));
    assert!(!acknowledged_abort(&doc! {}));
    assert!(!acknowledged_abort(
        &doc! {"ok": 1, "writeConcernError": {"code": 64}}
    ));
    assert!(!acknowledged_abort(
        &doc! {"ok": 1, "errorLabels": ["UnknownTransactionCommitResult"]}
    ));
}

async fn cancel_in_flight(
    db: &Database,
    app: &str,
    command: &str,
    code: i32,
    label: &str,
    caller: impl Future,
) {
    let admin = db.client().database("admin");
    let configured = admin
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
            "data": {"appName": app, "failCommands": [command], "blockConnection": true,
                "blockTimeMS": 500_i32, "errorCode": code, "errorLabels": [label]},
        })
        .await
        .unwrap();
    let before = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    let mut caller = Box::pin(caller);
    tokio::select! {
        _ = &mut caller => panic!("{command} completed before caller cancellation"),
        entered = admin.run_command(doc! {
            "waitForFailPoint": "failCommand", "timesEntered": before + 1, "maxTimeMS": 5_000,
        }) => { entered.unwrap(); }
    }
    drop(caller);
}

#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture with failpoints"]
async fn cancelled_migration_calls_keep_opening_and_terminal_outcomes_consistent() {
    let db = fixture().await;
    let mut options = ClientOptions::parse(std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let app = format!("migration_terminal_{}", uuid::Uuid::new_v4().simple());
    options.app_name = Some(app.clone());
    let worker = Client::with_options(options).unwrap().database(db.name());
    let runner = Runner::new(&worker, LEDGER, "fixture", &[]);
    let ledger = db.collection::<Document>(LEDGER);

    cancel_in_flight(
        &db,
        &app,
        "aggregate",
        2,
        "FixtureRefusal",
        runner.activate("fixture drained predecessor"),
    )
    .await;
    assert!(ledger
        .find_one(doc! {"_id": "activation"})
        .await
        .unwrap()
        .is_none());
    assert!(!ledger
        .find_one(doc! {"_id": "runner"})
        .await
        .unwrap()
        .unwrap()
        .get_bool("strict")
        .unwrap());

    // The first commit reply is lost. The owned terminal task retries that
    // commit on the original session even after the activation caller drops.
    cancel_in_flight(
        &db,
        &app,
        "commitTransaction",
        6,
        "UnknownTransactionCommitResult",
        runner.activate("fixture drained predecessor"),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while ledger
            .find_one(doc! {"_id": "activation"})
            .await
            .unwrap()
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the original commit did not settle after cancellation");
    assert_eq!(
        ledger
            .count_documents(doc! {"kind": "activation"})
            .await
            .unwrap(),
        1
    );
    runner.preflight().await.unwrap();

    let rows = db.collection::<Document>("migration_abort_control");
    rows.insert_one(doc! {"_id": "probe", "value": 0_i32})
        .await
        .unwrap();
    let worker_rows = worker.collection::<Document>(rows.name());
    let aborted = async {
        let mut session = start(&worker).await.unwrap();
        worker_rows
            .update_one(doc! {"_id": "probe"}, doc! {"$inc": {"value": 1_i32}})
            .session(&mut session)
            .await
            .unwrap();
        abort_after(session, "fixture", "original refusal").await;
    };
    cancel_in_flight(
        &db,
        &app,
        "abortTransaction",
        6,
        "RetryableWriteError",
        aborted,
    )
    .await;
    tokio::time::timeout(
        Duration::from_secs(3),
        rows.update_one(doc! {"_id": "probe"}, doc! {"$set": {"independent": true}})
            .into_future(),
    )
    .await
    .expect("the cancelled abort left its row locked")
    .unwrap();
    assert_eq!(
        rows.find_one(doc! {"_id": "probe"})
            .await
            .unwrap()
            .unwrap()
            .get_i32("value")
            .unwrap(),
        0
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture with failpoints"]
async fn server_abort_failure_is_reported_and_preserves_the_refusal() {
    let db = fixture().await;
    let mut options = ClientOptions::parse(std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let app = format!("migration_abort_{}", uuid::Uuid::new_v4().simple());
    options.app_name = Some(app.clone());
    let worker = Client::with_options(options).unwrap().database(db.name());
    let rows = worker.collection::<Document>("abort_probe");
    rows.insert_one(doc! {"_id": "fixture", "value": 0_i32})
        .await
        .unwrap();
    let mut session = start(&worker).await.unwrap();
    rows.update_one(doc! {"_id": "fixture"}, doc! {"$inc": {"value": 1_i32}})
        .session(&mut session)
        .await
        .unwrap();
    db.client()
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
            "data": {"appName": app, "failCommands": ["abortTransaction"], "errorCode": 2_i32},
        })
        .await
        .unwrap();

    let log = AbortLog::default();
    let refusal = abort_after(session, "claim", "original refusal")
        .with_subscriber(log.subscriber())
        .await;
    assert_eq!(refusal, "original refusal");
    assert!(log.contains("migration abort command failed"));
    assert!(log.contains("code=2"));
    let stored = db.collection::<Document>(rows.name());
    tokio::time::timeout(
        Duration::from_secs(3),
        stored
            .update_one(doc! {"_id": "fixture"}, doc! {"$set": {"settled": true}})
            .into_future(),
    )
    .await
    .expect("abort retry did not release the row")
    .unwrap();
    assert_eq!(
        stored
            .find_one(doc! {"_id": "fixture"})
            .await
            .unwrap()
            .unwrap()
            .get_i32("value")
            .unwrap(),
        0
    );
    db.drop().await.unwrap();
}
