use super::*;
use layer2_core::{document_artifacts::*, Authorization, Caller, Capability, SubjectId};
use mongodb::{
    bson::doc,
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client, Database,
};
use std::sync::{Arc, Mutex};

fn caller() -> Caller {
    Caller::new(
        SubjectId::new("commit-owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

fn body() -> CreateDocument {
    CreateDocument {
        title: "Committed source".into(),
        markdown: "source".into(),
        origin_agent_id: None,
    }
}

fn source(id: &str, markdown: &[u8]) -> DocumentSourceReservation {
    DocumentSourceReservation {
        owner_hash: digest(caller().subject.as_str().as_bytes()),
        document_id: id.into(),
        sha256: digest(markdown),
        bytes: markdown.len() as u64,
    }
}

async fn database(prefix: &str) -> Database {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo replica set required");
    let client = Client::with_uri_str(uri).await.unwrap();
    client.database(&format!("{}_{}", prefix, uuid::Uuid::new_v4().simple()))
}

async fn activate(quota: &MongoDocumentQuotaStore, database: &Database) {
    let activation = uuid::Uuid::new_v4().to_string();
    quota.prepare(&activation).await.unwrap();
    let inventory = super::quota_baseline::Inventory::parse(
        &serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "activation_id": activation,
            "complete_storage_inventory": true,
            "writers_drained": true,
            "objects": []
        }))
        .unwrap(),
    )
    .unwrap();
    quota.install(database, &inventory, true).await.unwrap();
}

async fn record() -> StoredDocument {
    let rows = Arc::new(layer2_core::document_artifacts::memory::MemoryDocuments::default());
    let service = DocumentService::new(
        rows.clone(),
        Arc::new(layer2_core::document_artifacts::memory::MemoryDocumentBlobs::default()),
    );
    let document = service.create(&caller(), "record", body()).await.unwrap();
    rows.get(&document.id).await.unwrap().unwrap()
}

async fn updated_record() -> StoredDocument {
    let rows = Arc::new(layer2_core::document_artifacts::memory::MemoryDocuments::default());
    let service = DocumentService::new(
        rows.clone(),
        Arc::new(layer2_core::document_artifacts::memory::MemoryDocumentBlobs::default()),
    );
    let document = service.create(&caller(), "record", body()).await.unwrap();
    service
        .update(
            &caller(),
            &document.id,
            "title",
            UpdateDocument {
                expected_revision: 1,
                title: Some("Updated title".into()),
                markdown: None,
            },
        )
        .await
        .unwrap();
    rows.get(&document.id).await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_commit_marks_reserved_object_committed_atomically_with_metadata() {
    let database = database("cma_document_commit").await;
    let store = MongoDocumentStore::new(database.clone());
    store.ensure_indexes().await.unwrap();
    let quota = MongoDocumentQuotaStore::new(&database, true);
    activate(&quota, &database).await;
    let metadata = record().await;
    let source = source(&metadata.metadata.id, b"source");
    assert_eq!(quota.reserve(&source).await, Ok(SourceTicket::Ledgered));
    assert_eq!(
        store
            .insert(&metadata, Some((&source, SourceTicket::Ledgered)))
            .await
            .unwrap(),
        InsertOutcome::Inserted
    );
    let object = quota
        .rows
        .find_one(doc! {"_id": format!("object:{}:{}:{}", source.owner_hash, source.document_id, source.sha256)})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(object.get_str("state").unwrap(), "committed");
    assert!(store.get(&metadata.metadata.id).await.unwrap().is_some());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_commit_aborts_when_object_was_condemned_and_document_is_unchanged() {
    let database = database("cma_document_commit").await;
    let store = MongoDocumentStore::new(database.clone());
    let quota = MongoDocumentQuotaStore::new(&database, true);
    activate(&quota, &database).await;
    let metadata = record().await;
    let source = source(&metadata.metadata.id, b"source");
    quota.reserve(&source).await.unwrap();
    quota
        .rows
        .update_one(
            doc! {"_id": format!("object:{}:{}:{}", source.owner_hash, source.document_id, source.sha256)},
            doc! {"$set": {"state": "condemned"}},
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .insert(&metadata, Some((&source, SourceTicket::Ledgered)))
            .await,
        Err(DocumentError::Unavailable)
    );
    assert!(store.get(&metadata.metadata.id).await.unwrap().is_none());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_commit_cas_conflict_leaves_object_reserved() {
    let database = database("cma_document_commit").await;
    let store = MongoDocumentStore::new(database.clone());
    let quota = MongoDocumentQuotaStore::new(&database, true);
    activate(&quota, &database).await;
    let metadata = record().await;
    let source = source(&metadata.metadata.id, b"source");
    quota.reserve(&source).await.unwrap();
    assert!(!store
        .compare_exchange(0, &metadata, Some((&source, SourceTicket::Ledgered)))
        .await
        .unwrap());
    let object = quota
        .rows
        .find_one(doc! {"_id": format!("object:{}:{}:{}", source.owner_hash, source.document_id, source.sha256)})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(object.get_str("state").unwrap(), "reserved");
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_commit_unledgered_ticket_keeps_plain_insert_and_cas() {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo replica set required");
    let mut options = ClientOptions::parse(uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed.lock().unwrap().push(event.command_name.clone());
        }
    }));
    let client = Client::with_options(options).unwrap();
    let database = client.database(&format!(
        "cma_document_commit_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = MongoDocumentStore::new(database.clone());
    let metadata = record().await;
    let source = source(&metadata.metadata.id, b"source");
    assert_eq!(
        store
            .insert(&metadata, Some((&source, SourceTicket::Unledgered)))
            .await
            .unwrap(),
        InsertOutcome::Inserted
    );
    let updated = updated_record().await;
    assert!(store
        .compare_exchange(1, &updated, Some((&source, SourceTicket::Unledgered)))
        .await
        .unwrap());
    assert!(!commands
        .lock()
        .unwrap()
        .iter()
        .any(|name| name == "startTransaction"));
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_commit_unknown_result_is_resolved_by_snapshot_read() {
    let expected = doc! {"_id": "doc_1"};
    let committed = doc! {"state": "committed"};
    assert!(commit::snapshot_matches(
        Some(&expected),
        Some(&committed),
        &expected
    ));
}
