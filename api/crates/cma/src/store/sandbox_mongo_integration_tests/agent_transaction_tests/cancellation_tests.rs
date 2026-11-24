//! A store transaction whose caller is cancelled during its first command must
//! not strand a write lock on the row it was about to change (#3217).
use crate::store::sandbox_mongo::{is_transient, start_transaction, MongoSandboxStore};
use mongodb::{bson::doc, bson::Document, options::ClientOptions, Client};
use std::time::{Duration, Instant};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn cancelled_first_command_of_a_store_transaction_strands_no_row_lock() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("isolated failpoint-enabled replica set");
    let name = format!("cma_txn_barrier_{}", uuid::Uuid::new_v4().simple());
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(name.clone());
    // Production clients carry their database; the barrier reads it.
    options.default_database = Some(name.clone());
    let client = Client::with_options(options).unwrap();
    MongoSandboxStore::open(&client, &name).await.unwrap();
    let rows = |client: &Client| {
        client
            .database(&name)
            .collection::<Document>("transaction_barrier_rows")
    };
    rows(&admin)
        .insert_one(doc! {"_id": "row", "revision": 0_i64})
        .await
        .unwrap();
    // Stall whichever command opens the transaction, then cancel the caller
    // while that command is in flight.
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
            "data": {
                "appName": &name, "failCommands": ["aggregate", "update"],
                "blockConnection": true, "blockTimeMS": 400_i32,
            },
        })
        .await
        .unwrap();
    {
        let mut session = client.start_session().await.unwrap();
        let caller = async {
            start_transaction(&mut session).await?;
            rows(&client)
                .update_one(doc! {"_id": "row"}, doc! {"$inc": {"revision": 1_i64}})
                .session(&mut session)
                .await
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(100), caller)
                .await
                .is_err(),
            "the stalled first command must be in flight when the caller is cancelled"
        );
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    // An independent client cannot reuse the dropped server session, so only
    // a lock left by the cancelled transaction could keep this write failing.
    let started = Instant::now();
    let mut conflicts = 0;
    loop {
        let mut session = admin.start_session().await.unwrap();
        session.start_transaction().await.unwrap();
        let written = rows(&admin)
            .update_one(doc! {"_id": "row"}, doc! {"$inc": {"revision": 10_i64}})
            .session(&mut session)
            .await;
        match written {
            Ok(_) => {
                session.commit_transaction().await.unwrap();
                break;
            }
            Err(error) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                conflicts += 1;
            }
            Err(error) => panic!("unexpected competing write error: {error}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a cancelled first command stranded the row lock ({conflicts} conflicts)"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let row = rows(&admin)
        .find_one(doc! {"_id": "row"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_i64("revision").unwrap(),
        10,
        "the cancelled caller's write never commits"
    );
    admin.database(&name).drop().await.unwrap();
}
