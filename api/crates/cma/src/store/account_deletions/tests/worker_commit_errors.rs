use super::worker_fixture::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unknown_commit_then_transient_failure_cannot_authorize_a_new_operation() {
    let (db, _, _, claim) = claimed().await;
    let app = format!(
        "erasure-unknown-transient-{}",
        uuid::Uuid::new_v4().simple()
    );
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let matched = app.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = Some(app);
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let CommandEvent::Started(event) = event else {
            return;
        };
        if event.command_name != "commitTransaction" {
            return;
        }
        let attempt = observed.fetch_add(1, Ordering::SeqCst);
        assert!(
            attempt < 2,
            "an uncertain command must not start a third commit sequence"
        );
        let mut data = doc! {"failCommands":["commitTransaction"],"appName":&matched};
        if attempt == 0 {
            data.insert(
                "writeConcernError",
                doc! {"code":50,"codeName":"MaxTimeMSExpired","errmsg":"synthetic unknown result"},
            );
        } else {
            data.insert("errorCode", 251);
            data.insert("errorLabels", vec!["TransientTransactionError"]);
        }
        // Synchronous command monitoring installs each scoped fault before the
        // corresponding wire send. A separate runtime avoids callback deadlock.
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async move {
                let admin=Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap()).await.unwrap();
                admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":data}).await.unwrap();
            });
        }).join().unwrap();
    }));
    let client = Client::with_options(options).unwrap();
    let journal = MongoErasureJournal::from_database(&client.database(db.name()));
    let request = plan(&claim, "retain-after-transient");
    let result = journal.commit(&claim, &request, &budget()).await;
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert!(
        matches!(result.unwrap(),JournalOutcome::RecoveryPending {operation} if operation==request.key())
    );
    let fresh = reopened(&db, None).await;
    let JournalResolution::Committed(receipt) =
        fresh.resolve(&request, None, &budget()).await.unwrap()
    else {
        panic!("first committed attempt must resolve")
    };
    assert_eq!(receipt.operation, request.key());
    assert_eq!(
        journal
            .operations
            .count_documents(doc! {"_id":request.key()})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}
