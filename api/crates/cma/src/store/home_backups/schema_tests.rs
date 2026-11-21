use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{Arc, Mutex};

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn explicit_catalog_open_refuses_missing_or_drifted_schema_without_repair_or_scan() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed = commands.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed.lock().unwrap().push(event.command_name);
        }
    }));
    let client = Client::with_options(options).unwrap();
    for case in [
        "absent",
        "ready",
        "missing_unique",
        "nonunique",
        "missing_query",
        "ttl",
        "validator",
        "missing_schedule",
    ] {
        let name = format!("home_backup_schema_{}", uuid::Uuid::new_v4().simple());
        let db = admin.database(&name);
        if case != "absent" {
            crate::store::migrations::home_backup_catalog::prepare_for_test(&db).await;
        }
        let backups = db.collection::<Document>(BACKUPS_COLLECTION);
        match case {
            "missing_unique" | "nonunique" => {
                backups.drop_index("sandbox_id_1_epoch_1").await.unwrap();
                if case == "nonunique" {
                    backups
                        .create_index(
                            mongodb::IndexModel::builder()
                                .keys(doc! {"sandbox_id":1,"epoch":1})
                                .build(),
                        )
                        .await
                        .unwrap();
                }
            }
            "missing_query" => {
                backups
                    .drop_index("sandbox_id_1_state_1_cut.ready_at_-1")
                    .await
                    .unwrap();
            }
            "ttl" => {
                backups
                    .create_index(
                        mongodb::IndexModel::builder()
                            .keys(doc! {"expiry":1})
                            .options(
                                mongodb::options::IndexOptions::builder()
                                    .expire_after(Duration::from_secs(1))
                                    .build(),
                            )
                            .build(),
                    )
                    .await
                    .unwrap();
            }
            "validator" => {
                db.run_command(
                    doc! {"collMod":BACKUPS_COLLECTION,"validator":{"unexpected":{"$exists":true}}},
                )
                .await
                .unwrap();
            }
            "missing_schedule" => {
                db.collection::<Document>(SCHEDULES_COLLECTION)
                    .drop()
                    .await
                    .unwrap();
            }
            _ => {}
        }
        let before = db.list_collection_names().await.unwrap();
        let result = MongoHomeBackupStore::open(&client, &name).await;
        assert_eq!(result.is_ok(), case == "ready", "{case}");
        assert_eq!(db.list_collection_names().await.unwrap(), before);
        db.drop().await.unwrap();
    }
    let commands = commands.lock().unwrap();
    assert!(!commands.is_empty());
    assert!(
        commands
            .iter()
            .all(|command| ["listCollections", "listIndexes"].contains(&command.as_str())),
        "{commands:?}"
    );
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn explicit_readers_preserve_retained_records_and_refuse_unknown_shapes() {
    let fixture = Fixture::new().await;
    let store = &fixture.store;
    let backup = standalone_backup(HomeBackupState::Creating);
    let schedule = schedule();
    let backup_row = backup_document(&backup).unwrap();
    let schedule_row = schedule_document(&schedule).unwrap();
    store.backups.insert_one(backup_row.clone()).await.unwrap();
    store
        .schedules
        .insert_one(schedule_row.clone())
        .await
        .unwrap();
    let reopened = MongoHomeBackupStore::open(&fixture.client, &fixture.database)
        .await
        .unwrap();
    assert_eq!(
        reopened.get_backup(&backup.id).await.unwrap(),
        Some(backup.clone())
    );
    assert_eq!(
        reopened.get_schedule(&schedule.id).await.unwrap(),
        Some(schedule.clone())
    );
    for (collection, mut row) in [
        (&store.backups, backup_row),
        (&store.schedules, schedule_row),
    ] {
        row.insert("unreviewed_field", true);
        collection
            .replace_one(doc! {"_id":row.get_str("_id").unwrap()}, row.clone())
            .await
            .unwrap();
        let rejected = if collection.name() == BACKUPS_COLLECTION {
            matches!(
                reopened.get_backup(&backup.id).await,
                Err(HomeBackupError::Invalid(_))
            )
        } else {
            matches!(
                reopened.get_schedule(&schedule.id).await,
                Err(HomeBackupError::Invalid(_))
            )
        };
        assert!(rejected);
        assert_eq!(collection.find_one(doc! {}).await.unwrap(), Some(row));
    }
    fixture.cleanup().await;
}
