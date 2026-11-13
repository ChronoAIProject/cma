use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_unknown_delete_and_release_commits_use_snapshot_readback() {
    let (db, _, service) = fixture().await;
    let document = created(&service).await;
    let application = format!("lifecycle-unknown-{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = Some(application.clone());
    let snapshots = Arc::new(AtomicUsize::new(0));
    let observed = snapshots.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find"
                && !event.command.contains_key("autocommit")
                && event
                    .command
                    .get_document("readConcern")
                    .ok()
                    .is_some_and(|concern| concern.get_str("level").ok() == Some("snapshot"))
            {
                observed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoDocumentStore::new(client.database(db.name())).deletion(true, true);
    let record = store.get(&document.id).await.unwrap().unwrap();
    ledger(&db, &record).await;
    let tombstone = DocumentTombstone {
        id: document.id.clone(),
        owner_hash: digest(caller().subject.as_str().as_bytes()),
        revision: 2,
        deleted_at: layer2_core::now_rfc3339(),
        receipt_key: digest(b"unknown-key"),
        fingerprint: digest(b"unknown-body"),
        objects: record
            .revisions
            .iter()
            .map(|r| r.object_key.clone())
            .collect(),
    };
    let admin = db.client().database("admin");
    let injection = doc! {"configureFailPoint":"failCommand","mode":{"times":5_i32},"data":{"failCommands":["commitTransaction"],"appName":application,"writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected lifecycle commit ambiguity"}}};
    admin.run_command(injection.clone()).await.unwrap();
    let result = store.delete(1, &tombstone).await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(result, Ok(DeleteOutcome::Deleted));
    assert!(snapshots.load(Ordering::SeqCst) > 0);
    let purge = store.purge();
    let lease = purge
        .claim(&document.id, &uuid::Uuid::new_v4().to_string(), 0)
        .await
        .unwrap()
        .unwrap();
    let before = snapshots.load(Ordering::SeqCst);
    admin.run_command(injection).await.unwrap();
    let result = purge.release(&lease).await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(result, Ok(true));
    assert!(snapshots.load(Ordering::SeqCst) > before);
    assert_eq!(purge.release(&lease).await, Ok(false));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_unknown_ledgered_create_commit_proves_all_rows_by_snapshot() {
    let (db, _, _) = fixture().await;
    let record = super::create::record().await;
    let source = ledger(&db, &record).await;
    let application = format!("lifecycle-create-unknown-{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = Some(application.clone());
    let snapshots = Arc::new(AtomicUsize::new(0));
    let observed = snapshots.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find"
                && !event.command.contains_key("autocommit")
                && event
                    .command
                    .get_document("readConcern")
                    .ok()
                    .is_some_and(|concern| concern.get_str("level").ok() == Some("snapshot"))
            {
                observed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoDocumentStore::new(client.database(db.name())).deletion(true, true);
    let admin = db.client().database("admin");
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":application,
        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected lifecycle create commit ambiguity"}}})
        .await
        .unwrap();
    let result = store
        .insert(&record, Some((&source, SourceTicket::Ledgered)))
        .await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(result, Ok(InsertOutcome::Inserted));
    assert!(
        snapshots.load(Ordering::SeqCst) >= 3,
        "an uncertain create must prove metadata, lifecycle and source in one snapshot"
    );
    assert!(store.get(&record.metadata.id).await.unwrap().is_some());
    let lifecycle = db
        .collection::<Document>(LIFECYCLE_COLLECTION)
        .find_one(doc! {"_id": &record.metadata.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lifecycle.get_str("state").unwrap(), "live");
    let object = db
        .collection::<Document>(QUOTA_COLLECTION)
        .find_one(doc! {"kind": "object"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(object.get_str("state").unwrap(), "committed");
    db.drop().await.unwrap();
}
