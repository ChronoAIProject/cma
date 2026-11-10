use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct RacingStore {
    rows: memory::MemoryDocuments,
    commits: AtomicUsize,
    barrier: tokio::sync::Barrier,
}

#[async_trait::async_trait]
impl DocumentStore for RacingStore {
    async fn get(&self, id: &str) -> Result<Option<StoredDocument>, DocumentError> {
        self.rows.get(id).await
    }
    async fn insert(&self, record: &StoredDocument) -> Result<bool, DocumentError> {
        self.rows.insert(record).await
    }
    async fn compare_exchange(
        &self,
        expected: u64,
        record: &StoredDocument,
    ) -> Result<bool, DocumentError> {
        if self.commits.fetch_add(1, Ordering::SeqCst) < 2 {
            self.barrier.wait().await;
        }
        self.rows.compare_exchange(expected, record).await
    }
    async fn list(
        &self,
        subject: &str,
        query: &DocumentQuery,
    ) -> Result<Vec<DocumentListing>, DocumentError> {
        self.rows.list(subject, query).await
    }
}

#[tokio::test]
async fn concurrent_exact_revoke_retries_commit_once_at_capacity() {
    let rows = Arc::new(RacingStore {
        rows: memory::MemoryDocuments::default(),
        commits: AtomicUsize::new(0),
        barrier: tokio::sync::Barrier::new(2),
    });
    let service = DocumentService::with_revocation_reserve(
        rows.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
        true,
    );
    let owner = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let guest = Caller::new(
        SubjectId::new("guest"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let doc = service
        .create(
            &owner,
            "create",
            CreateDocument {
                title: "Shared source".into(),
                markdown: "private".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    {
        let mut records = rows.rows.0.lock().await;
        let record = records.get_mut(&doc.id).unwrap();
        record.shares.push(DocumentShare {
            subject: "guest".into(),
            permission: DocumentPermission::Read,
        });
        for n in 0..MAX_RECEIPTS {
            record.receipts.insert(
                format!("{n:064x}"),
                DocumentReceipt {
                    fingerprint: format!("{n:064x}"),
                    outcome: DocumentMutation {
                        document: doc.clone(),
                        comment: None,
                        link: None,
                    },
                },
            );
        }
        record.validate().unwrap();
    }
    let (first, retry) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(
            service.revoke(&owner, &doc.id, "revoke", "guest", 1),
            service.revoke(&owner, &doc.id, "revoke", "guest", 1),
        )
    })
    .await
    .unwrap();
    assert_eq!(first.unwrap(), retry.unwrap());
    let record = rows.get(&doc.id).await.unwrap().unwrap();
    assert_eq!(record.metadata.revision, 2);
    assert_eq!(record.receipts.len(), MAX_RECEIPTS + 1);
    assert!(record.shares.is_empty());
    assert_eq!(
        service.source(&guest, &doc.id, None, None).await,
        Err(DocumentError::NotFound)
    );
    assert!(service.source(&owner, &doc.id, None, None).await.is_ok());
}
