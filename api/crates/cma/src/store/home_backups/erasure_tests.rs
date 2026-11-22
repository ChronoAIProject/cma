use super::*;
use crate::store::account_erasure::{erase_records, Cursor, Target};
use layer2_core::{account_deletion::ErasureInventory, SubjectId};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{Arc, Mutex};

async fn erase_ordinary_records(retained: bool) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("home_backup_erasure_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    let rows = [
        (
            SCHEDULES_COLLECTION,
            schedule_document(&schedule()).unwrap(),
        ),
        (
            BACKUPS_COLLECTION,
            backup_document(&standalone_backup(HomeBackupState::Ready)).unwrap(),
        ),
    ];
    if retained {
        for (collection, row) in &rows {
            db.collection(collection)
                .insert_one(row.clone())
                .await
                .unwrap();
        }
    }
    db.collection::<Document>("users")
        .insert_many([doc! {"_id":"acct:a"}, doc! {"_id":"acct:b"}])
        .await
        .unwrap();

    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed = commands.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed.lock().unwrap().push((event.db, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let subject = SubjectId::new("acct:a");
    let inventory = ErasureInventory {
        sandboxes: vec![schedule().id],
        ..Default::default()
    };
    let mut cursor = Cursor::default();
    let mut users_deleted = 0;
    let mut finished = false;
    for _ in 0..2000 {
        let batch = erase_records(
            &client.database(&name),
            &[Target {
                subject: &subject,
                inventory: &inventory,
            }],
            &mut cursor,
            "2026-11-20T00:00:00Z",
        )
        .await
        .unwrap();
        assert!(batch.discovered.is_empty());
        assert!(batch.pending_key_ids.is_empty());
        for (collection, _) in &rows {
            assert!(!batch.deleted.contains_key(*collection));
            assert!(!batch.redacted.contains_key(*collection));
        }
        users_deleted += batch.deleted.get("users").copied().unwrap_or(0);
        if !batch.has_more {
            finished = true;
            break;
        }
        cursor = mongodb::bson::deserialize_from_document(
            mongodb::bson::serialize_to_document(&cursor).unwrap(),
        )
        .unwrap();
    }
    assert!(finished, "ordinary record erasure did not finish");
    assert_eq!(users_deleted, 1);
    assert_eq!(
        db.collection::<Document>("users")
            .find_one(doc! {})
            .await
            .unwrap(),
        Some(doc! {"_id":"acct:b"})
    );
    let backup_commands: Vec<_> = commands
        .lock()
        .unwrap()
        .iter()
        .filter(|(database, command)| {
            database == &name
                && rows
                    .iter()
                    .any(|(collection, _)| command.to_string().contains(collection))
        })
        .cloned()
        .collect();
    assert!(backup_commands.is_empty(), "{backup_commands:?}");
    let names = db.list_collection_names().await.unwrap();
    for (collection, row) in rows {
        if retained {
            assert_eq!(
                db.collection::<Document>(collection)
                    .find_one(doc! {})
                    .await
                    .unwrap(),
                Some(row)
            );
            assert_eq!(
                db.collection::<Document>(collection)
                    .list_indexes()
                    .await
                    .unwrap()
                    .try_collect::<Vec<_>>()
                    .await
                    .unwrap()
                    .len(),
                1
            );
        } else {
            assert!(!names.iter().any(|name| name == collection));
        }
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn ordinary_account_erasure_leaves_absent_backup_namespaces_untouched() {
    erase_ordinary_records(false).await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn ordinary_account_erasure_preserves_backup_object_custody() {
    erase_ordinary_records(true).await;
}
