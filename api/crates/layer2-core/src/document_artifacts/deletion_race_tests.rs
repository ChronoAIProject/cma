use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{Barrier, Semaphore};

pub(super) struct DeletionRaceStore {
    rows: memory::MemoryDocuments,
    loaded: Barrier,
    winner_finished: Semaphore,
    delete_first: bool,
    deletes: AtomicUsize,
    exchanges: AtomicUsize,
    delete_expected: AtomicU64,
    exchange_expected: AtomicU64,
}
impl DeletionRaceStore {
    pub(super) fn new(delete_first: bool) -> Self {
        Self {
            rows: Default::default(),
            loaded: Barrier::new(2),
            winner_finished: Semaphore::new(0),
            delete_first,
            deletes: 0.into(),
            exchanges: 0.into(),
            delete_expected: 0.into(),
            exchange_expected: 0.into(),
        }
    }
    async fn before_commit(&self, deleting: bool) {
        // Both requests have loaded the live aggregate before either may commit.
        self.loaded.wait().await;
        if deleting != self.delete_first {
            self.winner_finished.acquire().await.unwrap().forget();
        }
    }
    fn after_commit(&self, deleting: bool) {
        if deleting == self.delete_first {
            self.winner_finished.add_permits(1);
        }
    }
    pub(super) fn assert_both_loaded_revision_one(&self) {
        assert_eq!(self.deletes.load(Ordering::SeqCst), 1);
        assert_eq!(self.exchanges.load(Ordering::SeqCst), 1);
        assert_eq!(self.delete_expected.load(Ordering::SeqCst), 1);
        assert_eq!(self.exchange_expected.load(Ordering::SeqCst), 1);
    }
}
#[async_trait::async_trait]
impl DocumentStore for DeletionRaceStore {
    async fn get(&self, id: &str) -> Result<Option<StoredDocument>, DocumentError> {
        self.rows.get(id).await
    }
    async fn insert(
        &self,
        record: &StoredDocument,
        source: Option<(&DocumentSourceReservation, SourceTicket)>,
    ) -> Result<InsertOutcome, DocumentError> {
        self.rows.insert(record, source).await
    }
    async fn delete(
        &self,
        expected: u64,
        tombstone: &DocumentTombstone,
    ) -> Result<DeleteOutcome, DocumentError> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        self.delete_expected.store(expected, Ordering::SeqCst);
        self.before_commit(true).await;
        let result = self.rows.delete(expected, tombstone).await;
        self.after_commit(true);
        result
    }
    async fn compare_exchange(
        &self,
        expected: u64,
        record: &StoredDocument,
        source: Option<(&DocumentSourceReservation, SourceTicket)>,
    ) -> Result<bool, DocumentError> {
        self.exchanges.fetch_add(1, Ordering::SeqCst);
        self.exchange_expected.store(expected, Ordering::SeqCst);
        self.before_commit(false).await;
        let result = self.rows.compare_exchange(expected, record, source).await;
        self.after_commit(false);
        result
    }
    async fn tombstone(&self, id: &str) -> Result<Option<DocumentTombstone>, DocumentError> {
        self.rows.tombstone(id).await
    }
    async fn purge_state(&self, id: &str) -> Result<Option<DocumentPurgeState>, DocumentError> {
        self.rows.purge_state(id).await
    }
    async fn pending_deletions(&self, owner_hash: &str) -> Result<u64, DocumentError> {
        self.rows.pending_deletions(owner_hash).await
    }
    async fn list(
        &self,
        subject: &str,
        query: &DocumentQuery,
    ) -> Result<Vec<DocumentListing>, DocumentError> {
        self.rows.list(subject, query).await
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
        title: "Race report".into(),
        markdown: "# Retained".into(),
        origin_agent_id: None,
    }
}
async fn mutation(service: &DocumentService, id: &str, kind: u8) -> Result<u64, DocumentError> {
    let owner = owner();
    match kind {
        0 => Ok(service
            .update(
                &owner,
                id,
                "update",
                UpdateDocument {
                    expected_revision: 1,
                    title: Some("Updated title".into()),
                    markdown: None,
                },
            )
            .await?
            .revision),
        1 => Ok(service
            .grant(
                &owner,
                id,
                "grant",
                "guest",
                GrantDocument {
                    expected_revision: 1,
                    permission: DocumentPermission::Comment,
                },
            )
            .await?
            .revision),
        _ => Ok(service
            .comment(
                &owner,
                id,
                None,
                "comment",
                CreateDocumentComment {
                    body: "Concurrent comment".into(),
                    document_revision: None,
                },
            )
            .await?
            .document
            .revision),
    }
}

#[tokio::test]
async fn delete_races_update_grant_and_comment_exactly_one_wins() {
    for kind in 0..3 {
        for delete_first in [true, false] {
            let rows = Arc::new(DeletionRaceStore::new(delete_first));
            let service = DocumentService::with_deletion(
                rows.clone(),
                Arc::new(memory::MemoryDocumentBlobs::default()),
                false,
                None,
                true,
            );
            let owner = owner();
            let doc = service.create(&owner, "create", create()).await.unwrap();
            let delete = service.delete(
                &owner,
                &doc.id,
                "delete",
                DeleteDocument {
                    expected_revision: 1,
                },
            );
            let mutate = mutation(&service, &doc.id, kind);
            let (deleted, mutated) =
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    tokio::join!(delete, mutate)
                })
                .await
                .unwrap();
            rows.assert_both_loaded_revision_one();
            assert_eq!(
                usize::from(deleted.is_ok()) + usize::from(mutated.is_ok()),
                1
            );
            if delete_first {
                assert_eq!(deleted.unwrap().revision, 2);
                assert_eq!(mutated, Err(DocumentError::NotFound));
                assert!(rows.get(&doc.id).await.unwrap().is_none());
                assert!(rows.tombstone(&doc.id).await.unwrap().is_some());
            } else {
                assert_eq!(deleted, Err(DocumentError::Conflict));
                assert_eq!(mutated, Ok(2));
                assert!(rows.tombstone(&doc.id).await.unwrap().is_none());
                assert_eq!(
                    rows.get(&doc.id).await.unwrap().unwrap().metadata.revision,
                    2
                );
            }
        }
    }
}

struct HeldSource {
    inner: memory::MemoryDocumentBlobs,
    hold: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl DocumentBlobStore for HeldSource {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        self.inner.put(key, bytes).await
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, DocumentError> {
        let bytes = self.inner.get(key).await?;
        if self.hold.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(bytes)
    }
}

#[tokio::test]
async fn deletion_while_source_read_is_in_flight_prevents_delivery() {
    let blobs = Arc::new(HeldSource {
        inner: Default::default(),
        hold: false.into(),
        entered: Default::default(),
        release: Default::default(),
    });
    let service = DocumentService::with_deletion(
        Arc::new(memory::MemoryDocuments::default()),
        blobs.clone(),
        false,
        None,
        true,
    );
    let owner = owner();
    let doc = service.create(&owner, "create", create()).await.unwrap();
    blobs.hold.store(true, Ordering::SeqCst);
    let task = tokio::spawn({
        let service = service.clone();
        let id = doc.id.clone();
        async move {
            service
                .source(&super::deletion_race_tests::owner(), &id, None, None)
                .await
        }
    });
    blobs.entered.notified().await;
    service
        .delete(
            &owner,
            &doc.id,
            "delete",
            DeleteDocument {
                expected_revision: 1,
            },
        )
        .await
        .unwrap();
    blobs.release.notify_one();
    assert_eq!(task.await.unwrap(), Err(DocumentError::NotFound));
}
