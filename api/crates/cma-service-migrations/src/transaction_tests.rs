use super::*;
use crate::tests::{fixture, LEDGER};
use crate::Runner;
use mongodb::{bson::Document, options::ClientOptions, Client};
use std::{
    future::{Future, IntoFuture},
    time::Duration,
};

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
    let mut options = ClientOptions::parse(std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap())
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
        abort(session).await;
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
