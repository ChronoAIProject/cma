#[path = "deletion_mongo_tests.rs"]
mod deletion_tests;
use super::*;
use crate::document_artifacts::{
    source_commit::{reconcile_after_majority_read, source_marker},
    MongoDocumentStore, COLLECTION,
};
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::Mutex;
use tokio::sync::Barrier;

#[derive(Default)]
struct Blobs {
    store: memory::MemoryDocumentBlobs,
    pause: tokio::sync::Mutex<Option<Arc<Barrier>>>,
    pending_failpoint: tokio::sync::Mutex<Option<(Database, CommitFault)>>,
}

enum CommitFault {
    LostAcknowledgement,
    Unresolved,
}

#[async_trait::async_trait]
impl DocumentBlobStore for Blobs {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        self.store.put(key, bytes).await?;
        let barrier = self.pause.lock().await.clone();
        if let Some(barrier) = barrier {
            barrier.wait().await;
        }
        if let Some((admin, fault)) = self.pending_failpoint.lock().await.take() {
            // The reservation has committed by the time object storage is called.
            let mut data = match fault {
                CommitFault::LostAcknowledgement => doc! {
                    "writeConcernError": { "code": 64, "errmsg": "fixture lost acknowledgement" },
                },
                CommitFault::Unresolved => doc! {
                    "errorCode": 1,
                    "errorLabels": ["UnknownTransactionCommitResult"],
                },
            };
            data.insert("failCommands", vec!["commitTransaction"]);
            admin
                .run_command(doc! {
                    "configureFailPoint": "failCommand",
                    "mode": "alwaysOn",
                    "data": data,
                })
                .await
                .unwrap();
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, DocumentError> {
        self.store.get(key).await
    }
}

fn owner() -> Caller {
    Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

fn create() -> CreateDocument {
    CreateDocument {
        title: "Original".into(),
        markdown: "original".into(),
        origin_agent_id: None,
    }
}

fn edit(expected_revision: u64, title: &str, source: Option<&str>) -> UpdateDocument {
    UpdateDocument {
        expected_revision,
        title: Some(title.into()),
        markdown: source.map(str::to_owned),
    }
}

async fn settle_injected_commit(admin: &Database, database: &Database) {
    admin
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    crate::store::test_visibility::majority_barrier(database.client(), database.name()).await;
}

fn service(store: &MongoDocumentStore, blobs: Arc<Blobs>, quotas: bool) -> Arc<DocumentService> {
    DocumentService::with_storage_quota(
        Arc::new(store.clone()),
        blobs,
        false,
        Some(Arc::new(store.quota(quotas))),
    )
}

fn reservation(id: &str, body: &str) -> DocumentSourceReservation {
    DocumentSourceReservation {
        document_id: id.into(),
        sha256: digest(body.as_bytes()),
        bytes: body.len() as u64,
        owner_hash: digest(b"owner"),
    }
}

async fn marker(quota: &MongoDocumentQuotaStore, id: &str, body: &str) -> Option<Document> {
    let source = reservation(id, body);
    let expected = source_marker(&source);
    quota
        .rows
        .find_one(doc! { "_id": &expected["_id"] })
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_source_commits_require_a_ready_baseline_before_put() {
    let database = database().await;
    let store = MongoDocumentStore::new(database.clone());
    let enabled = store.with_source_commits(true);
    let blobs = Arc::new(Blobs::default());
    let owner = owner();
    for quotas in [false, true] {
        assert_eq!(
            service(&enabled, blobs.clone(), quotas)
                .create(&owner, "source", create())
                .await,
            Err(DocumentError::Unavailable)
        );
    }
    let quota = enabled.quota(true);
    quota
        .prepare(&uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    assert_eq!(
        service(&enabled, blobs.clone(), true)
            .create(&owner, "source", create())
            .await,
        Err(DocumentError::Unavailable)
    );
    assert!(blobs.store.0.lock().await.is_empty());
    assert_eq!(
        database
            .collection::<Document>(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_source_commits_preserve_predecessor_rows_and_title_only_edits() {
    let database = database().await;
    let predecessor = MongoDocumentStore::new(database.clone());
    predecessor.ensure_indexes().await.unwrap();
    let enabled = predecessor.with_source_commits(true);
    let quota = enabled.quota(true);
    activate(&quota, &database, &[]).await;
    let blobs = Arc::new(Blobs::default());
    let owner = owner();
    let legacy = service(&predecessor, blobs.clone(), true);
    let current = service(&enabled, blobs.clone(), true);
    let historical = legacy.create(&owner, "legacy", create()).await.unwrap();
    current
        .update(&owner, &historical.id, "title", edit(1, "Rename", None))
        .await
        .unwrap();
    assert_eq!(marker(&quota, &historical.id, "original").await, None);

    let created = current.create(&owner, "current", create()).await.unwrap();
    let source = reservation(&created.id, "original");
    let charged = object_row(&source).unwrap();
    assert_eq!(
        quota
            .rows
            .find_one(doc! { "_id": &charged["_id"] })
            .await
            .unwrap(),
        Some(charged)
    );
    assert_eq!(
        marker(&quota, &created.id, "original").await,
        Some(source_marker(&source))
    );
    predecessor.quota(true).reserve(&source).await.unwrap();
    assert_eq!(
        legacy.read(&owner, &created.id, None).await.unwrap(),
        created
    );
    assert_eq!(usage(&quota, "owner").await, [2, 16, 2]);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_source_commits_distinguish_losing_and_identical_uploads() {
    let database = database().await;
    let store = MongoDocumentStore::new(database.clone()).with_source_commits(true);
    store.ensure_indexes().await.unwrap();
    let quota = store.quota(true);
    activate(&quota, &database, &[]).await;
    let blobs = Arc::new(Blobs::default());
    let service = service(&store, blobs.clone(), true);
    let owner = owner();
    let created = service.create(&owner, "create", create()).await.unwrap();

    for (revision, sources) in [(1, ["different-a", "different-b"]), (3, ["same", "same"])] {
        *blobs.pause.lock().await = Some(Arc::new(Barrier::new(2)));
        let keys = [format!("edit-{revision}-a"), format!("edit-{revision}-b")];
        let edits = [
            edit(revision, "First", Some(sources[0])),
            edit(revision, "Second", Some(sources[1])),
        ];
        let (first, second) = tokio::join!(
            service.update(&owner, &created.id, &keys[0], edits[0].clone()),
            service.update(&owner, &created.id, &keys[1], edits[1].clone()),
        );
        *blobs.pause.lock().await = None;
        let (winner, loser, outcome) = match (first, second) {
            (Ok(result), Err(DocumentError::Conflict)) => (0, 1, result),
            (Err(DocumentError::Conflict), Ok(result)) => (1, 0, result),
            other => panic!("expected one winning edit: {other:?}"),
        };
        for (i, body) in sources.into_iter().enumerate() {
            let source = reservation(&created.id, body);
            let charged = object_row(&source).unwrap();
            assert_eq!(
                quota
                    .rows
                    .find_one(doc! { "_id": &charged["_id"] })
                    .await
                    .unwrap(),
                Some(charged)
            );
            assert_eq!(
                marker(&quota, &created.id, body).await,
                (i == winner || sources[0] == sources[1]).then(|| source_marker(&source))
            );
        }
        service
            .update(
                &owner,
                &created.id,
                &format!("advance-{revision}"),
                edit(revision + 1, "Advance", None),
            )
            .await
            .unwrap();
        assert_eq!(
            service
                .update(&owner, &created.id, &keys[winner], edits[winner].clone())
                .await
                .unwrap(),
            outcome
        );
        assert_eq!(
            service
                .update(&owner, &created.id, &keys[loser], edits[loser].clone())
                .await,
            Err(DocumentError::Conflict)
        );
    }
    assert_eq!(usage(&quota, "owner").await, [1, 34, 4]);
    let reused = service
        .update(
            &owner,
            &created.id,
            "repeat-source",
            edit(5, "Reuse", Some("same")),
        )
        .await
        .unwrap();
    assert_eq!(reused.revision, 6);
    assert_eq!(usage(&quota, "owner").await, [1, 34, 4]);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set with test failpoints"]
async fn document_quota_source_commits_reconcile_uncertain_creation_and_edit() {
    let database = database().await;
    let admin = database.client().database("admin");
    let store = MongoDocumentStore::new(database.clone()).with_source_commits(true);
    store.ensure_indexes().await.unwrap();
    let quota = store.quota(true);
    activate(&quota, &database, &[]).await;
    let blobs = Arc::new(Blobs::default());
    let service = service(&store, blobs.clone(), true);
    let owner = owner();
    *blobs.pending_failpoint.lock().await = Some((admin.clone(), CommitFault::LostAcknowledgement));
    let first = service.create(&owner, "create", create()).await;
    settle_injected_commit(&admin, &database).await;
    assert_eq!(
        quota
            .rows
            .count_documents(doc! { "kind": "source_commit" })
            .await
            .unwrap(),
        1
    );
    let created = service.create(&owner, "create", create()).await.unwrap();
    match first {
        Ok(response) => assert_eq!(response, created),
        Err(error) => assert_eq!(error, DocumentError::Unavailable),
    }
    assert!(marker(&quota, &created.id, "original").await.is_some());

    let body = edit(1, "Edited", Some("uncertain"));
    *blobs.pending_failpoint.lock().await = Some((admin.clone(), CommitFault::Unresolved));
    let unresolved = service
        .update(&owner, &created.id, "edit", body.clone())
        .await;
    settle_injected_commit(&admin, &database).await;
    assert_eq!(unresolved, Err(DocumentError::Unavailable));
    assert_eq!(
        service.read(&owner, &created.id, None).await.unwrap(),
        created
    );
    assert!(marker(&quota, &created.id, "uncertain").await.is_none());
    assert_eq!(usage(&quota, "owner").await, [1, 17, 2]);
    let updated = service
        .update(&owner, &created.id, "edit", body.clone())
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(
        service
            .update(&owner, &created.id, "edit", body)
            .await
            .unwrap(),
        updated
    );
    assert!(marker(&quota, &created.id, "uncertain").await.is_some());
    assert_eq!(usage(&quota, "owner").await, [1, 17, 2]);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_source_commits_recover_the_winning_attempt_timestamps() {
    let database = database().await;
    let starts = Arc::new(Mutex::new(Vec::new()));
    let observed = starts.clone();
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_bool("startTransaction").ok() == Some(true) {
                observed.lock().unwrap().push(event.command_name);
            }
        }
    }));
    let database = Client::with_options(options)
        .unwrap()
        .database(database.name());
    let store = MongoDocumentStore::new(database.clone()).with_source_commits(true);
    store.ensure_indexes().await.unwrap();
    let quota = store.quota(true);
    activate(&quota, &database, &[]).await;
    let service = service(&store, Arc::new(Blobs::default()), true);
    let owner = owner();
    starts.lock().unwrap().clear();
    let created = service.create(&owner, "create", create()).await.unwrap();
    // Reservation reads the durable generation gate before publication opens
    // its own read-first transaction.
    assert_eq!(*starts.lock().unwrap(), ["find", "find"]);
    let mut alternate = store.get(&created.id).await.unwrap().unwrap();
    let timestamp = "2026-01-01T00:00:00Z";
    alternate.metadata.created_at = timestamp.into();
    alternate.metadata.updated_at = timestamp.into();
    alternate.revisions[0].metadata.created_at = timestamp.into();
    assert_ne!(created, alternate.metadata);
    assert!(!reconcile_after_majority_read(
        &store,
        &alternate,
        &reservation(&created.id, "original"),
        None,
    )
    .await
    .unwrap());
    assert_eq!(
        service.create(&owner, "create", create()).await.unwrap(),
        created
    );

    let body = edit(1, "Edited", Some("replacement"));
    starts.lock().unwrap().clear();
    let updated = service
        .update(&owner, &created.id, "edit", body.clone())
        .await
        .unwrap();
    assert_eq!(*starts.lock().unwrap(), ["find", "find"]);
    let mut alternate = store.get(&created.id).await.unwrap().unwrap();
    alternate.metadata.updated_at = timestamp.into();
    alternate.revisions.last_mut().unwrap().metadata.created_at = timestamp.into();
    let key = alternate.receipts.keys().next().unwrap().clone();
    alternate.receipts.get_mut(&key).unwrap().outcome.document = alternate.metadata.clone();
    assert_ne!(updated, alternate.metadata);
    assert!(!reconcile_after_majority_read(
        &store,
        &alternate,
        &reservation(&created.id, "replacement"),
        Some((1, &key)),
    )
    .await
    .unwrap());
    assert_eq!(
        service
            .update(&owner, &created.id, "edit", body)
            .await
            .unwrap(),
        updated
    );
    assert_eq!(usage(&quota, "owner").await, [1, 19, 2]);
    assert_eq!(
        quota
            .rows
            .count_documents(doc! { "kind": "source_commit" })
            .await
            .unwrap(),
        2
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_deletion_fences_metadata_and_preserves_the_charged_reclaim_obligation() {
    let database = database().await;
    let store = MongoDocumentStore::new(database.clone()).with_source_commits(true);
    store.ensure_indexes().await.unwrap();
    let quota = store.quota(true);
    activate(&quota, &database, &[]).await;
    let blobs = Arc::new(Blobs::default());
    let service = service(&store, blobs.clone(), true).with_deletion_enabled(true);
    let owner = owner();
    let doc = service.create(&owner, "create", create()).await.unwrap();
    let before = usage(&quota, "owner").await;
    let live = store.get(&doc.id).await.unwrap().unwrap();
    let (left, right) = tokio::join!(
        service.delete(&owner, &doc.id, "delete", 1),
        service.delete(&owner, &doc.id, "delete", 1)
    );
    assert_eq!(left.unwrap(), right.unwrap());
    let tombstone = store.tombstone(&doc.id).await.unwrap().unwrap();
    assert_eq!(tombstone.receipt.revision, 2);
    assert!(store.get(&doc.id).await.unwrap().is_none());
    assert!(!store.insert(&live).await.unwrap());
    assert_eq!(usage(&quota, "owner").await, before);
    assert_eq!(
        quota.reserve(&reservation(&doc.id, "new source")).await,
        Err(DocumentError::NotFound)
    );
    let pending = quota.rows.find_one(doc! {"_id": crate::document_artifacts::deletion::obligation_id(&tombstone.owner_hash, &doc.id)}).await.unwrap().unwrap();
    assert_eq!(
        pending.get_str("request_hash").unwrap(),
        tombstone.request_hash
    );
    assert!(blobs
        .store
        .0
        .lock()
        .await
        .contains_key(&reservation(&doc.id, "original").object_key()));
    database.drop().await.unwrap();
}
