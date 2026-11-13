use super::*;
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Metadata {
    rows: memory::MemoryDocuments,
    fail_insert: AtomicBool,
    ambiguous_edit: AtomicBool,
}
#[async_trait::async_trait]
impl DocumentStore for Metadata {
    async fn get(&self, id: &str) -> Result<Option<StoredDocument>, DocumentError> {
        self.rows.get(id).await
    }
    async fn insert(
        &self,
        record: &StoredDocument,
        source: Option<(&DocumentSourceReservation, SourceTicket)>,
    ) -> Result<InsertOutcome, DocumentError> {
        if self.fail_insert.load(Ordering::SeqCst) {
            return Err(DocumentError::Unavailable);
        }
        self.rows.insert(record, source).await
    }
    async fn delete(
        &self,
        expected: u64,
        tombstone: &DocumentTombstone,
    ) -> Result<DeleteOutcome, DocumentError> {
        self.rows.delete(expected, tombstone).await
    }
    async fn tombstone(&self, id: &str) -> Result<Option<DocumentTombstone>, DocumentError> {
        self.rows.tombstone(id).await
    }
    async fn purge_state(&self, id: &str) -> Result<Option<DocumentPurgeState>, DocumentError> {
        self.rows.purge_state(id).await
    }
    async fn pending_deletions(&self, owner: &str) -> Result<u64, DocumentError> {
        self.rows.pending_deletions(owner).await
    }
    async fn compare_exchange(
        &self,
        expected: u64,
        record: &StoredDocument,
        source: Option<(&DocumentSourceReservation, SourceTicket)>,
    ) -> Result<bool, DocumentError> {
        let committed = self.rows.compare_exchange(expected, record, source).await?;
        if self.ambiguous_edit.load(Ordering::SeqCst) {
            Err(DocumentError::Unavailable)
        } else {
            Ok(committed)
        }
    }
    async fn list(
        &self,
        subject: &str,
        query: &DocumentQuery,
    ) -> Result<Vec<DocumentListing>, DocumentError> {
        self.rows.list(subject, query).await
    }
}
#[derive(Default)]
struct Blobs {
    rows: memory::MemoryDocumentBlobs,
    fail: AtomicBool,
    hold: AtomicBool,
    written: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl DocumentBlobStore for Blobs {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        self.rows.put(key, bytes).await?;
        self.written.notify_one();
        if self.hold.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self.fail.load(Ordering::SeqCst) {
            Err(DocumentError::Unavailable)
        } else {
            Ok(())
        }
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, DocumentError> {
        self.rows.get(key).await
    }
}
fn caller() -> Caller {
    Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
fn create() -> CreateDocument {
    CreateDocument {
        title: "Quota source".into(),
        markdown: "source".into(),
        origin_agent_id: None,
    }
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_preserves_failed_upload_insert_and_ambiguous_edit_without_double_charge() {
    let database = database().await;
    let mut quota = MongoDocumentQuotaStore::new(&database, true);
    quota.limits = [1, 12, 2];
    activate(&quota, &database, &[]).await;
    let quota = Arc::new(quota);
    let blobs = Arc::new(Blobs::default());
    let metadata = Arc::new(Metadata::default());
    let service = DocumentService::with_storage_quota(
        metadata.clone(),
        blobs.clone(),
        true,
        Some(quota.clone()),
    );
    blobs.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        service.create(&caller(), "original", create()).await,
        Err(DocumentError::Unavailable)
    );
    assert_eq!(usage(&quota, "owner").await, [1, 6, 1]);
    assert_eq!(
        service.create(&caller(), "new-identity", create()).await,
        Err(DocumentError::LimitReached {
            dimension: DocumentLimit::Documents,
            limit: 1,
            live: 1
        })
    );
    assert_eq!(blobs.rows.0.lock().await.len(), 1);
    blobs.fail.store(false, Ordering::SeqCst);
    metadata.fail_insert.store(true, Ordering::SeqCst);
    assert_eq!(
        service.create(&caller(), "original", create()).await,
        Err(DocumentError::Unavailable)
    );
    assert_eq!(usage(&quota, "owner").await, [1, 6, 1]);
    metadata.fail_insert.store(false, Ordering::SeqCst);
    let created = service
        .create(&caller(), "original", create())
        .await
        .unwrap();
    metadata.ambiguous_edit.store(true, Ordering::SeqCst);
    let edit = UpdateDocument {
        expected_revision: 1,
        title: None,
        markdown: Some("edited".into()),
    };
    assert_eq!(
        service
            .update(&caller(), &created.id, "edit", edit.clone())
            .await,
        Err(DocumentError::Unavailable)
    );
    assert_eq!(usage(&quota, "owner").await, [1, 12, 2]);
    let replay = service
        .update(&caller(), &created.id, "edit", edit)
        .await
        .unwrap();
    assert_eq!(replay.revision, 2);
    assert_eq!(usage(&quota, "owner").await, [1, 12, 2]);
    assert_eq!(blobs.rows.0.lock().await.len(), 2);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_cancellation_after_object_write_never_releases_capacity() {
    let database = database().await;
    let mut quota = MongoDocumentQuotaStore::new(&database, true);
    quota.limits = [1, 6, 1];
    activate(&quota, &database, &[]).await;
    let quota = Arc::new(quota);
    let blobs = Arc::new(Blobs::default());
    let metadata = Arc::new(Metadata::default());
    blobs.hold.store(true, Ordering::SeqCst);
    let service = DocumentService::with_storage_quota(
        metadata.clone(),
        blobs.clone(),
        true,
        Some(quota.clone()),
    );
    let running = service.clone();
    let task = tokio::spawn(async move { running.create(&caller(), "cancelled", create()).await });
    tokio::time::timeout(Duration::from_secs(5), blobs.written.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(metadata.rows.0.lock().await.is_empty());
    assert_eq!(usage(&quota, "owner").await, [1, 6, 1]);
    assert_eq!(
        service.create(&caller(), "another", create()).await,
        Err(DocumentError::LimitReached {
            dimension: DocumentLimit::Documents,
            limit: 1,
            live: 1
        })
    );
    blobs.hold.store(false, Ordering::SeqCst);
    service
        .create(&caller(), "cancelled", create())
        .await
        .unwrap();
    assert_eq!(usage(&quota, "owner").await, [1, 6, 1]);
    database.drop().await.unwrap();
}
