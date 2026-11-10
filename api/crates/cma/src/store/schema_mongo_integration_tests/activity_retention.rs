use std::sync::{Arc, Mutex};

use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions};

use super::*;

const RETENTION: Duration = Duration::from_secs(90 * 86_400);

async fn observed_client(uri: &str, app: &str) -> (Client, Arc<Mutex<Vec<String>>>) {
    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed = commands.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(app.to_owned());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed.lock().unwrap().push(event.command_name);
        }
    }));
    (Client::with_options(options).unwrap(), commands)
}

fn assert_no_retention_mutation(commands: &Arc<Mutex<Vec<String>>>) {
    let commands = commands.lock().unwrap();
    for forbidden in ["collMod", "dropIndexes", "drop", "delete", "update"] {
        assert!(
            !commands.iter().any(|name| name == forbidden),
            "{commands:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn fresh_and_matching_activity_retention_open_repeatedly() {
    let (client, name) = database().await;
    MongoActivityStore::open(&client, &name, RETENTION)
        .await
        .unwrap();
    let db = client.database(&name);
    let original = index_documents(&db, ACTIVITY).await;
    let expiry = original
        .iter()
        .find(|row| row.get_str("name").ok() == Some("activity_expiry"))
        .unwrap();
    assert_eq!(
        expiry.get_document("key").unwrap(),
        &doc! { "recorded_at": 1 }
    );
    assert_eq!(
        expiry
            .get("expireAfterSeconds")
            .unwrap()
            .as_i64()
            .or_else(|| expiry
                .get("expireAfterSeconds")
                .unwrap()
                .as_i32()
                .map(i64::from)),
        Some(RETENTION.as_secs() as i64)
    );
    for _ in 0..2 {
        MongoActivityStore::open(&client, &name, RETENTION)
            .await
            .unwrap();
        assert_eq!(index_documents(&db, ACTIVITY).await, original);
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn activity_retention_drift_preserves_policy_and_records() {
    let (admin, name) = database().await;
    MongoActivityStore::open(&admin, &name, RETENTION)
        .await
        .unwrap();
    let db = admin.database(&name);
    let original = index_documents(&db, ACTIVITY).await;
    let record = doc! { "_id": "retained", "document_schema_version": 1_i64,
    "recorded_at": DateTime::now(), "record": { "fixture": "retained" } };
    insert(&admin, &name, ACTIVITY, record.clone()).await;
    let (client, commands) =
        observed_client(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(), &name).await;
    for retention in [
        Duration::from_secs(30 * 86_400),
        Duration::from_secs(180 * 86_400),
    ] {
        let result = MongoActivityStore::open(&client, &name, retention).await;
        assert!(
            matches!(result, Err(StoreError::Unavailable(message)) if message.contains("explicit migration required"))
        );
        assert_eq!(index_documents(&db, ACTIVITY).await, original);
        assert_eq!(
            db.collection::<Document>(ACTIVITY)
                .find_one(doc! {})
                .await
                .unwrap(),
            Some(record.clone())
        );
    }
    MongoActivityStore::open(&client, &name, RETENTION)
        .await
        .unwrap();
    assert_no_retention_mutation(&commands);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn conflicting_activity_expiry_index_refuses_without_repair() {
    let (admin, name) = database().await;
    let db = admin.database(&name);
    db.collection::<Document>(ACTIVITY)
        .create_index(
            IndexModel::builder()
                .keys(doc! { "wrong_timestamp": 1 })
                .options(
                    IndexOptions::builder()
                        .name("activity_expiry".to_owned())
                        .expire_after(RETENTION)
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    let before = index_documents(&db, ACTIVITY)
        .await
        .into_iter()
        .find(|row| row.get_str("name").ok() == Some("activity_expiry"))
        .unwrap();
    let (client, commands) =
        observed_client(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(), &name).await;
    assert!(MongoActivityStore::open(&client, &name, RETENTION)
        .await
        .is_err());
    let after = index_documents(&db, ACTIVITY)
        .await
        .into_iter()
        .find(|row| row.get_str("name").ok() == Some("activity_expiry"))
        .unwrap();
    assert_eq!(before, after);
    assert_no_retention_mutation(&commands);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn activity_expiry_permission_failure_never_authorizes_collmod() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_activity_failure_{}", uuid::Uuid::new_v4().simple());
    MongoActivityStore::open(&admin, &name, RETENTION)
        .await
        .unwrap();
    let db = admin.database(&name);
    let original = index_documents(&db, ACTIVITY).await;
    let (client, commands) = observed_client(&uri, &name).await;
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "skip": 2_i32 },
            "data": { "failCommands": ["createIndexes"], "appName": &name, "errorCode": 13_i32 }
        })
        .await
        .unwrap();
    let result = MongoActivityStore::open(&client, &name, RETENTION).await;
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": "off"
        })
        .await
        .expect("disable failpoint before assertions");
    assert!(
        matches!(result, Err(StoreError::Unavailable(message)) if message.contains("explicit migration required"))
    );
    assert_eq!(
        commands
            .lock()
            .unwrap()
            .iter()
            .filter(|name| name.as_str() == "createIndexes")
            .count(),
        3
    );
    assert_no_retention_mutation(&commands);
    assert_eq!(index_documents(&db, ACTIVITY).await, original);
    MongoActivityStore::open(&client, &name, RETENTION)
        .await
        .unwrap();
    db.drop().await.unwrap();
}
