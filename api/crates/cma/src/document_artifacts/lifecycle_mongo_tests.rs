use super::*;
use layer2_core::{document_artifacts::*, Authorization, Caller, Capability, SubjectId};
use mongodb::{
    bson::{doc, DateTime, Document},
    Client, Database,
};
use std::sync::Arc;

mod create;
mod purge;
mod reclaim;
mod unknown;

fn caller() -> Caller {
    Caller::new(
        SubjectId::new("lifecycle-owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
async fn fixture() -> (Database, Arc<MongoDocumentStore>, Arc<DocumentService>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let db = Client::with_uri_str(uri)
        .await
        .unwrap()
        .database(&format!("cma_lifecycle_{}", uuid::Uuid::new_v4().simple()));
    for name in [COLLECTION, QUOTA_COLLECTION, LIFECYCLE_COLLECTION] {
        db.create_collection(name).await.unwrap();
    }
    let store = Arc::new(MongoDocumentStore::new(db.clone()).deletion(true, true));
    store.ensure_indexes().await.unwrap();
    let service = DocumentService::with_deletion(
        store.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
        false,
        None,
        true,
    );
    (db, store, service)
}
async fn created(service: &DocumentService) -> DocumentView {
    service
        .create(
            &caller(),
            "create",
            CreateDocument {
                title: "Lifecycle fixture".into(),
                markdown: "# Content".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap()
}
async fn deleted(service: &DocumentService, id: &str, revision: u64) {
    service
        .delete(
            &caller(),
            id,
            "delete",
            DeleteDocument {
                expected_revision: revision,
            },
        )
        .await
        .unwrap();
}
async fn ledger(db: &Database, record: &StoredDocument) -> DocumentSourceReservation {
    let revision = &record.revisions[0];
    let source = DocumentSourceReservation {
        owner_hash: digest(caller().subject.as_str().as_bytes()),
        document_id: record.metadata.id.clone(),
        sha256: revision.metadata.sha256.clone(),
        bytes: revision.metadata.size_bytes,
    };
    let rows = db.collection::<Document>(QUOTA_COLLECTION);
    rows.insert_one(doc! {"_id":format!("owner:{}",source.owner_hash),"document_schema_version":1_i64,"kind":"owner","fence":0_i64,"documents":1_i64,"bytes":source.bytes as i64,"objects":1_i64}).await.unwrap();
    rows.insert_one(super::quota::document_row(&source))
        .await
        .unwrap();
    rows.insert_one(super::quota::object_row(&source).unwrap())
        .await
        .unwrap();
    source
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_deleted_id_is_never_recreated_by_late_create_with_same_key() {
    let (db, store, service) = fixture().await;
    let document = created(&service).await;
    let stale = store.get(&document.id).await.unwrap().unwrap();
    deleted(&service, &document.id, 1).await;
    assert_eq!(store.insert(&stale, None).await, Ok(InsertOutcome::Deleted));
    assert_eq!(
        service
            .create(
                &caller(),
                "create",
                CreateDocument {
                    title: "Lifecycle fixture".into(),
                    markdown: "# Content".into(),
                    origin_agent_id: None
                }
            )
            .await,
        Err(DocumentError::NotFound)
    );
    assert!(store.get(&document.id).await.unwrap().is_none());
    let raw = db
        .collection::<Document>(LIFECYCLE_COLLECTION)
        .find_one(doc! {"_id":&document.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.get_i64("document_schema_version").unwrap(), 1);
    assert_eq!(raw.get_str("state").unwrap(), "deleting");
    for content in ["lifecycle-owner", "Lifecycle fixture", "# Content"] {
        assert!(
            !format!("{raw:?}").contains(content),
            "tombstones retain no document content or raw subject"
        );
    }
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_deletion_flag_off_process_cannot_recreate_tombstoned_id() {
    let (db, store, service) = fixture().await;
    let document = created(&service).await;
    let stale = store.get(&document.id).await.unwrap().unwrap();
    deleted(&service, &document.id, 1).await;
    let off = MongoDocumentStore::new(db.clone());
    assert_eq!(off.insert(&stale, None).await, Ok(InsertOutcome::Deleted));
    assert!(off.get(&document.id).await.unwrap().is_none());
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_delete_races_update_grant_comment_exactly_one_wins() {
    for mutation in ["update", "grant", "comment"] {
        let (db, store, service) = fixture().await;
        let document = created(&service).await;
        let owner = caller();
        let remove = service.delete(
            &owner,
            &document.id,
            "delete",
            DeleteDocument {
                expected_revision: 1,
            },
        );
        let edit = async {
            match mutation {
                "update" => service
                    .update(
                        &owner,
                        &document.id,
                        "update",
                        UpdateDocument {
                            expected_revision: 1,
                            title: Some("Updated title".into()),
                            markdown: None,
                        },
                    )
                    .await
                    .map(|_| ()),
                "grant" => service
                    .grant(
                        &owner,
                        &document.id,
                        "grant",
                        "shared-reader",
                        GrantDocument {
                            expected_revision: 1,
                            permission: DocumentPermission::Read,
                        },
                    )
                    .await
                    .map(|_| ()),
                _ => service
                    .comment(
                        &owner,
                        &document.id,
                        None,
                        "comment",
                        CreateDocumentComment {
                            body: "Comment fixture".into(),
                            document_revision: Some(1),
                        },
                    )
                    .await
                    .map(|_| ()),
            }
        };
        let (remove, write) = tokio::join!(remove, edit);
        assert_eq!(usize::from(remove.is_ok()) + usize::from(write.is_ok()), 1);
        assert!(remove.is_ok() || matches!(remove, Err(DocumentError::Conflict)));
        if remove.is_ok() {
            assert!(store.get(&document.id).await.unwrap().is_none());
        }
        db.drop().await.unwrap();
    }
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_reserve_after_delete_is_refused() {
    let (db, store, service) = fixture().await;
    let document = created(&service).await;
    let source = ledger(&db, &store.get(&document.id).await.unwrap().unwrap()).await;
    let rows = db.collection::<Document>(QUOTA_COLLECTION);
    rows.insert_one(doc! {"_id":"activation","kind":"activation","document_schema_version":1_i64,"phase":"ready","inventory_sha256":digest(b"inventory"),"activation_id":uuid::Uuid::new_v4().to_string(),"baseline_rows":0_i64,"next_row":0_i64}).await.unwrap();
    deleted(&service, &document.id, 1).await;
    let quota = MongoDocumentQuotaStore::new(&db, true);
    assert_eq!(
        quota.reserve(&source).await,
        Err(DocumentError::NotFound),
        "exact existing object must also be fenced"
    );
    assert_eq!(
        quota
            .reserve(&DocumentSourceReservation {
                sha256: digest(b"later"),
                ..source
            })
            .await,
        Err(DocumentError::NotFound)
    );
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_rollback_reader_sees_deleted_document_as_absent() {
    let (db, _, service) = fixture().await;
    let document = created(&service).await;
    deleted(&service, &document.id, 1).await;
    let old = db
        .collection::<Document>(COLLECTION)
        .find_one(doc! {"_id":document.id})
        .await
        .unwrap();
    assert_eq!(
        old.as_ref()
            .map(super::mongo::decode)
            .transpose()
            .unwrap()
            .map(|r| r.metadata.id),
        None
    );
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_usage_reports_pending_deletions() {
    let (db, store, service) = fixture().await;
    let document = created(&service).await;
    assert_eq!(
        service.usage(&caller()).await.unwrap().pending_deletions,
        Some(0)
    );
    deleted(&service, &document.id, 1).await;
    assert_eq!(
        service.usage(&caller()).await.unwrap().pending_deletions,
        Some(1)
    );
    assert_eq!(
        store.pending_deletions(&digest(b"different-owner")).await,
        Ok(0)
    );
    let off = DocumentService::new(store, Arc::new(memory::MemoryDocumentBlobs::default()));
    assert_eq!(off.usage(&caller()).await.unwrap().pending_deletions, None);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_unsupported_storage_is_blocked_at_logical_deletion() {
    let (db, _, service) = fixture().await;
    let document = created(&service).await;
    let store = Arc::new(MongoDocumentStore::new(db.clone()).deletion(true, false));
    let unsupported = DocumentService::with_deletion(
        store.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
        false,
        None,
        true,
    );
    let result = unsupported
        .delete(
            &caller(),
            &document.id,
            "delete",
            DeleteDocument {
                expected_revision: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.purge, DocumentPurgeState::Blocked);
    assert!(store.get(&document.id).await.unwrap().is_none());
    let row = db
        .collection::<Document>(LIFECYCLE_COLLECTION)
        .find_one(doc! {"_id":document.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_document("purge")
            .unwrap()
            .get_str("blocked")
            .unwrap(),
        "storage_deletion_unsupported"
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_delete_revision_overflow_preserves_live_document_and_quota() {
    let (db, store, service) = fixture().await;
    let mut record = create::record().await;
    record.metadata.revision = i64::MAX as u64;
    record.validate().unwrap();
    let encoded = super::mongo::encode(&record).unwrap();
    assert_eq!(
        store.insert(&record, None).await,
        Ok(InsertOutcome::Inserted)
    );
    ledger(&db, &record).await;
    let lifecycle = db.collection::<Document>(LIFECYCLE_COLLECTION);
    let before = lifecycle
        .find_one(doc! {"_id": &record.metadata.id})
        .await
        .unwrap()
        .unwrap();
    let rows = db.collection::<Document>(QUOTA_COLLECTION);
    let owner = rows
        .find_one(doc! {"kind": "owner"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        service
            .delete(
                &caller(),
                &record.metadata.id,
                "delete-at-revision-limit",
                DeleteDocument {
                    expected_revision: i64::MAX as u64,
                },
            )
            .await,
        Err(DocumentError::Corrupt)
    );
    assert_eq!(
        db.collection::<Document>(COLLECTION)
            .find_one(doc! {"_id": &record.metadata.id})
            .await
            .unwrap(),
        Some(encoded)
    );
    assert_eq!(
        lifecycle
            .find_one(doc! {"_id": &record.metadata.id})
            .await
            .unwrap(),
        Some(before),
        "an unrepresentable tombstone revision must preserve the live identity"
    );
    assert_eq!(
        rows.find_one(doc! {"kind": "owner"}).await.unwrap(),
        Some(owner)
    );
    assert_eq!(
        rows.count_documents(doc! {"kind": "object"}).await.unwrap(),
        1
    );
    assert_eq!(
        rows.count_documents(doc! {"kind": "document"})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}
