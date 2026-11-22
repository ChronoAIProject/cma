use super::worker_fixture::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unprepared_journal_and_wrong_indexes_never_attempt_runtime_ddl() {
    use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions};
    let (_, db, store) = fixture().await;
    let record = store
        .request(
            &SubjectId::new("owner"),
            "accepted",
            &database_now(&db).await,
        )
        .await
        .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let monitor = count.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if ["create", "createIndexes", "collMod", "dropIndexes"]
                .contains(&event.command_name.as_str())
            {
                monitor.fetch_add(1, Ordering::SeqCst);
            }
        }
    }));
    let client = mongodb::Client::with_options(options).unwrap();
    let journal = MongoErasureJournal::from_database(&client.database(db.name()));
    let request = JournalRequest::new(binding(record.id), JournalCommand::Enroll).unwrap();
    assert!(matches!(
        journal.enroll(&request, &budget()).await,
        Err(JournalError::Unprepared)
    ));
    assert_eq!(count.load(Ordering::SeqCst), 0);
    crate::store::migrations::account_erasure_work::prepare_for_test(&db).await;
    let effects = db.collection::<Document>(worker_schema::EFFECTS);
    effects
        .drop_index("erasure_operation_sequence_v1")
        .await
        .unwrap();
    effects
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"deletion_id":1,"journal_sequence":-1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name("erasure_operation_sequence_v1".to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    assert!(matches!(
        journal.enroll(&request, &budget()).await,
        Err(JournalError::Incompatible)
    ));
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(
        db.collection::<Document>(worker_schema::WORK)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn preparation_receipt_unknown_versions_and_active_ttl_are_required_boundaries() {
    let (db, _, journal, claim) = claimed().await;
    let request = plan(&claim, "refused");
    let manifests = db.collection::<Document>("agent_manager_migrations");
    let saved = manifests
        .find_one(doc! {"migration":"am-account-erasure-work-v1","kind":"manifest"})
        .await
        .unwrap()
        .unwrap();
    manifests
        .delete_one(doc! {"_id":saved.get("_id").unwrap().clone()})
        .await
        .unwrap();
    assert!(matches!(
        journal.commit(&claim, &request, &budget()).await,
        Err(JournalError::Unprepared)
    ));
    manifests.insert_one(saved).await.unwrap();
    let id = claim.checkpoint().binding.deletion_id.as_str();
    journal
        .checkpoints
        .update_one(doc! {"_id":id}, doc! {"$set":{"schema":2}})
        .await
        .unwrap();
    assert!(matches!(
        journal.commit(&claim, &request, &budget()).await,
        Err(JournalError::Incompatible)
    ));
    journal
        .checkpoints
        .update_one(doc! {"_id":id}, doc! {"$set":{"schema":1}})
        .await
        .unwrap();
    journal
        .operations
        .insert_one(doc! {"_id":"unknown-kind", "schema":1, "kind":"future"})
        .await
        .unwrap();
    assert!(matches!(
        journal.commit(&claim, &request, &budget()).await,
        Err(JournalError::Incompatible)
    ));
    journal
        .operations
        .delete_one(doc! {"_id":"unknown-kind"})
        .await
        .unwrap();
    journal
        .operations
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"unsafe_expiry":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .expire_after(Duration::ZERO)
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    let before = snapshot(&db).await;
    assert!(matches!(
        journal.commit(&claim, &request, &budget()).await,
        Err(JournalError::Incompatible)
    ));
    assert_eq!(snapshot(&db).await, before);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operation_identity_and_sequence_are_unique_without_ttl() {
    let (db, _, journal, claim) = claimed().await;
    let request = plan(&claim, "one");
    journal.commit(&claim, &request, &budget()).await.unwrap();
    let mut row = journal
        .operations
        .find_one(doc! {"_id":request.key()})
        .await
        .unwrap()
        .unwrap();
    assert!(journal.operations.insert_one(row.clone()).await.is_err());
    row.insert("_id", "different-operation");
    assert!(journal.operations.insert_one(row).await.is_err());
    for name in worker_schema::COLLECTIONS {
        let indexes = db
            .collection::<Document>(name)
            .list_indexes()
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert!(indexes.iter().all(|index| index
            .options
            .as_ref()
            .is_none_or(|options| options.expire_after.is_none())));
    }
    db.drop().await.unwrap();
}
