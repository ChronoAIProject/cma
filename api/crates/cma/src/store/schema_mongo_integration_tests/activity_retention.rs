use std::sync::{Arc, Mutex};

use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions};

use super::*;
use crate::{config::StartupSchemaMode, store::schema::StartupObserver};

pub(super) const RETENTION: Duration = Duration::from_secs(90 * 86_400);

pub(super) async fn observed_client(uri: &str, app: &str) -> (Client, Arc<Mutex<Vec<String>>>) {
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

pub(super) fn assert_no_schema_mutation(commands: &Arc<Mutex<Vec<String>>>) {
    let commands = commands.lock().unwrap();
    for forbidden in [
        "create",
        "createIndexes",
        "collMod",
        "dropIndexes",
        "drop",
        "insert",
        "delete",
        "update",
        "findAndModify",
    ] {
        assert!(
            !commands.iter().any(|name| name == forbidden),
            "{commands:?}"
        );
    }
}

pub(super) async fn prepare_activity_schema(database: &Database, retention: Duration) {
    // Only this disposable fixture authorizes its TTL. Startup never does.
    database
        .collection::<Document>(ACTIVITY)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"recorded_at":1})
                .options(
                    IndexOptions::builder()
                        .name("activity_expiry".to_owned())
                        .expire_after(retention)
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    for keys in [
        doc! {"record.actor":1,"recorded_at":-1,"_id":-1},
        doc! {"record.action":1,"recorded_at":-1,"_id":-1},
    ] {
        database
            .collection::<Document>(ACTIVITY)
            .create_index(IndexModel::builder().keys(keys).build())
            .await
            .unwrap();
    }
}

pub(super) async fn open_activity(
    client: &Client,
    name: &str,
    retention: Duration,
    mode: StartupSchemaMode,
) -> Result<MongoActivityStore, StoreError> {
    MongoActivityStore::open_with_schema(client, name, retention, &mut StartupObserver::new(mode))
        .await
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn prepared_activity_retention_opens_repeatedly_without_any_ddl() {
    let (admin, name) = database().await;
    let db = admin.database(&name);
    prepare_activity_schema(&db, RETENTION).await;
    let original = index_documents(&db, ACTIVITY).await;
    let (client, commands) =
        observed_client(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(), &name).await;
    for mode in [StartupSchemaMode::Observe, StartupSchemaMode::Enforce] {
        for _ in 0..2 {
            open_activity(&client, &name, RETENTION, mode)
                .await
                .unwrap();
            assert_eq!(index_documents(&db, ACTIVITY).await, original);
        }
    }
    assert_no_schema_mutation(&commands);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn activity_retention_drift_observes_or_refuses_without_mutation() {
    let (admin, name) = database().await;
    let db = admin.database(&name);
    prepare_activity_schema(&db, RETENTION).await;
    let original = index_documents(&db, ACTIVITY).await;
    let record = doc! {"_id":"retained","document_schema_version":1_i64,"recorded_at":DateTime::now(),"record":{"fixture":"retained"}};
    insert(&admin, &name, ACTIVITY, record.clone()).await;
    let (client, commands) =
        observed_client(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(), &name).await;
    for retention in [
        Duration::from_secs(30 * 86_400),
        Duration::from_secs(180 * 86_400),
    ] {
        open_activity(&client, &name, retention, StartupSchemaMode::Observe)
            .await
            .unwrap();
        assert!(
            matches!(open_activity(&client, &name, retention, StartupSchemaMode::Enforce).await,
            Err(StoreError::Unavailable(message)) if message.contains("explicit migration"))
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
    assert_no_schema_mutation(&commands);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn conflicting_activity_expiry_index_is_observed_or_refused_without_repair() {
    let (admin, name) = database().await;
    let db = admin.database(&name);
    db.collection::<Document>(ACTIVITY)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"wrong_timestamp":1})
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
    let before = index_documents(&db, ACTIVITY).await;
    let (client, commands) =
        observed_client(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(), &name).await;
    open_activity(&client, &name, RETENTION, StartupSchemaMode::Observe)
        .await
        .unwrap();
    assert!(
        open_activity(&client, &name, RETENTION, StartupSchemaMode::Enforce)
            .await
            .is_err()
    );
    assert_eq!(index_documents(&db, ACTIVITY).await, before);
    assert_no_schema_mutation(&commands);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn activity_metadata_permission_failure_is_observed_or_refused_without_ddl() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_activity_failure_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    prepare_activity_schema(&db, RETENTION).await;
    let original = index_documents(&db, ACTIVITY).await;
    let (client, commands) = observed_client(&uri, &name).await;
    for mode in [StartupSchemaMode::Observe, StartupSchemaMode::Enforce] {
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand","mode":{"times":1_i32},
                "data":{"failCommands":["listIndexes"],"appName":&name,"errorCode":13_i32}
            })
            .await
            .unwrap();
        let result = open_activity(&client, &name, RETENTION, mode).await;
        admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .expect("disable failpoint before assertions");
        assert_eq!(result.is_ok(), mode == StartupSchemaMode::Observe);
    }
    assert_no_schema_mutation(&commands);
    assert_eq!(index_documents(&db, ACTIVITY).await, original);
    open_activity(&client, &name, RETENTION, StartupSchemaMode::Enforce)
        .await
        .unwrap();
    db.drop().await.unwrap();
}
