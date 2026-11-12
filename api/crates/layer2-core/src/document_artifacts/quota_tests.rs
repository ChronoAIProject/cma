use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::Mutex;

#[derive(Default)]
struct Quota {
    denied: AtomicBool,
    unavailable: AtomicBool,
    reservations: Mutex<Vec<DocumentSourceReservation>>,
}
#[async_trait::async_trait]
impl DocumentQuotaStore for Quota {
    async fn reserve(
        &self,
        source: &DocumentSourceReservation,
    ) -> Result<SourceTicket, DocumentError> {
        source.validate()?;
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(DocumentError::Unavailable);
        }
        if self.denied.load(Ordering::SeqCst) {
            return Err(DocumentError::Capacity);
        }
        self.reservations.lock().await.push(source.clone());
        Ok(SourceTicket::Ledgered)
    }
}
struct Blobs {
    quota: Arc<Quota>,
    rows: memory::MemoryDocumentBlobs,
    puts: AtomicUsize,
    fail: AtomicBool,
}
#[async_trait::async_trait]
impl DocumentBlobStore for Blobs {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        assert!(self
            .quota
            .reservations
            .lock()
            .await
            .iter()
            .any(|r| r.object_key() == key && r.bytes == bytes.len() as u64));
        self.puts.fetch_add(1, Ordering::SeqCst);
        self.rows.put(key, bytes).await?;
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
fn owner() -> Caller {
    Caller::new(
        SubjectId::new("quota-owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
fn body() -> CreateDocument {
    CreateDocument {
        title: "Quota".into(),
        markdown: "source".into(),
        origin_agent_id: None,
    }
}
fn fixture() -> (Arc<DocumentService>, Arc<Quota>, Arc<Blobs>) {
    let quota = Arc::new(Quota::default());
    let blobs = Arc::new(Blobs {
        quota: quota.clone(),
        rows: Default::default(),
        puts: 0.into(),
        fail: false.into(),
    });
    let service = DocumentService::with_storage_quota(
        Arc::new(memory::MemoryDocuments::default()),
        blobs.clone(),
        true,
        Some(quota.clone()),
    );
    (service, quota, blobs)
}

#[tokio::test]
async fn quota_precedes_create_and_edit_put_and_denial_keeps_reads_revoke_and_replay_available() {
    let (service, quota, blobs) = fixture();
    let owner = owner();
    let document = service.create(&owner, "create", body()).await.unwrap();
    let edited = service
        .update(
            &owner,
            &document.id,
            "edit",
            UpdateDocument {
                expected_revision: 1,
                markdown: Some("edited".into()),
                title: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(edited.revision, 2);
    assert_eq!(blobs.puts.load(Ordering::SeqCst), 2);
    quota.denied.store(true, Ordering::SeqCst);
    assert_eq!(
        service.create(&owner, "another", body()).await,
        Err(DocumentError::Capacity)
    );
    assert_eq!(
        service
            .update(
                &owner,
                &document.id,
                "denied",
                UpdateDocument {
                    expected_revision: 2,
                    markdown: Some("more".into()),
                    title: None
                }
            )
            .await,
        Err(DocumentError::Capacity)
    );
    assert_eq!(blobs.puts.load(Ordering::SeqCst), 2);
    assert!(service.create(&owner, "create", body()).await.is_ok());
    service
        .update(
            &owner,
            &document.id,
            "title",
            UpdateDocument {
                expected_revision: 2,
                title: Some("Renamed".into()),
                markdown: None,
            },
        )
        .await
        .unwrap();
    service
        .grant(
            &owner,
            &document.id,
            "grant",
            "quota-reader",
            GrantDocument {
                expected_revision: 3,
                permission: DocumentPermission::Read,
            },
        )
        .await
        .unwrap();
    service
        .revoke(&owner, &document.id, "revoke", "quota-reader", 4)
        .await
        .unwrap();
    let link = service
        .create_link(
            &owner,
            &document.id,
            "link",
            CreateDocumentLink {
                expected_revision: 5,
                token: digest(b"quota-link-fixture"),
                permission: DocumentPermission::Read,
            },
        )
        .await
        .unwrap()
        .link
        .unwrap();
    service
        .revoke_link(&owner, &document.id, "unlink", &link.id, 6)
        .await
        .unwrap();
    assert_eq!(
        service
            .source(&owner, &document.id, None, None)
            .await
            .unwrap()
            .markdown,
        "edited"
    );
    assert_eq!(quota.reservations.lock().await.len(), 2);
}

#[tokio::test]
async fn denied_principal_foreign_edit_invalid_input_and_stale_revision_never_reserve() {
    let (service, quota, _) = fixture();
    let owner = owner();
    let no_permission = Caller::new(SubjectId::new("no-permission"), Authorization::new([]));
    assert_eq!(
        service.create(&no_permission, "create", body()).await,
        Err(DocumentError::NotFound)
    );
    let mut invalid = body();
    invalid.markdown.clear();
    assert_eq!(
        service.create(&owner, "invalid", invalid).await,
        Err(DocumentError::Invalid)
    );
    assert!(quota.reservations.lock().await.is_empty());
    let document = service.create(&owner, "create", body()).await.unwrap();
    let foreign = Caller::new(
        SubjectId::new("foreign-owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let update = UpdateDocument {
        expected_revision: 1,
        markdown: Some("foreign".into()),
        title: None,
    };
    assert_eq!(
        service
            .update(&foreign, &document.id, "edit", update.clone())
            .await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service
            .update(
                &owner,
                &document.id,
                "edit",
                UpdateDocument {
                    expected_revision: 0,
                    ..update
                }
            )
            .await,
        Err(DocumentError::Conflict)
    );
    assert_eq!(quota.reservations.lock().await.len(), 1);
}

#[tokio::test]
async fn unavailable_quota_rejects_before_blob_upload() {
    let (service, quota, blobs) = fixture();
    quota.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        service.create(&owner(), "unavailable", body()).await,
        Err(DocumentError::Unavailable)
    );
    assert_eq!(blobs.puts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ambiguous_blob_result_retains_the_original_reservation_and_original_identity() {
    let (service, quota, blobs) = fixture();
    blobs.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        service.create(&owner(), "create", body()).await,
        Err(DocumentError::Unavailable)
    );
    let reserved = quota.reservations.lock().await[0].clone();
    assert_eq!(blobs.rows.0.lock().await.len(), 1);
    blobs.fail.store(false, Ordering::SeqCst);
    let created = service.create(&owner(), "create", body()).await.unwrap();
    assert_eq!(created.id, reserved.document_id);
    assert_eq!(
        quota.reservations.lock().await.as_slice(),
        &[reserved.clone(), reserved]
    );
    assert_eq!(blobs.rows.0.lock().await.len(), 1);
}

#[tokio::test]
async fn title_only_update_and_metadata_mutations_pass_no_source_to_the_store() {
    let (service, quota, _) = fixture();
    let owner = owner();
    let document = service.create(&owner, "create", body()).await.unwrap();
    let source = quota.reservations.lock().await[0].clone();
    assert_eq!(
        service
            .update(
                &owner,
                &document.id,
                "title",
                UpdateDocument {
                    expected_revision: 1,
                    title: Some("Renamed".into()),
                    markdown: None,
                },
            )
            .await
            .unwrap()
            .revision,
        2
    );
    service
        .grant(
            &owner,
            &document.id,
            "grant",
            "reader",
            GrantDocument {
                expected_revision: 2,
                permission: DocumentPermission::Read,
            },
        )
        .await
        .unwrap();
    service
        .revoke(&owner, &document.id, "revoke", "reader", 3)
        .await
        .unwrap();
    assert_eq!(
        service
            .source(&owner, &document.id, None, None)
            .await
            .unwrap()
            .markdown,
        "source"
    );
    assert_eq!(quota.reservations.lock().await.len(), 1);
    assert_eq!(source.bytes, 6);
}

struct BlockingBlobs {
    rows: memory::MemoryDocumentBlobs,
    block: AtomicBool,
    started: AtomicUsize,
    release: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl DocumentBlobStore for BlockingBlobs {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        if self.block.load(Ordering::SeqCst) {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.release.notified().await;
        }
        self.rows.put(key, bytes).await
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, DocumentError> {
        self.rows.get(key).await
    }
}

#[tokio::test]
async fn source_write_slots_refuse_without_queueing() {
    let blobs = Arc::new(BlockingBlobs {
        rows: Default::default(),
        block: false.into(),
        started: 0.into(),
        release: Default::default(),
    });
    let rows = Arc::new(memory::MemoryDocuments::default());
    let service = DocumentService::new(rows, blobs.clone());
    let owner = owner();
    let existing = service.create(&owner, "existing", body()).await.unwrap();
    blobs.block.store(true, Ordering::SeqCst);
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..DOCUMENT_SOURCE_WRITE_SLOTS {
        let service = service.clone();
        let owner = owner.clone();
        tasks.spawn(async move { service.create(&owner, &format!("source-{n}"), body()).await });
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while blobs.started.load(Ordering::SeqCst) < DOCUMENT_SOURCE_WRITE_SLOTS {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        service.create(&owner, "source-17", body()).await,
        Err(DocumentError::Busy)
    );
    assert!(service
        .update(
            &owner,
            &existing.id,
            "title-only",
            UpdateDocument {
                expected_revision: 1,
                title: Some("Still available".into()),
                markdown: None,
            },
        )
        .await
        .is_ok());
    blobs.block.store(false, Ordering::SeqCst);
    blobs.release.notify_waiters();
    let mut successes = 0;
    while let Some(result) = tasks.join_next().await {
        assert!(result.unwrap().is_ok());
        successes += 1;
    }
    assert_eq!(successes, DOCUMENT_SOURCE_WRITE_SLOTS);
}

#[tokio::test]
async fn commit_failure_after_condemned_object_leaves_metadata_unchanged() {
    let rows = memory::MemoryDocuments::default();
    let source =
        DocumentSourceReservation::new("owner", &format!("doc_{}", "a".repeat(32)), b"source");
    let service = DocumentService::new(
        Arc::new(memory::MemoryDocuments::default()),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    );
    let record = service.create(&owner(), "record", body()).await.unwrap();
    let record = service.store.get(&record.id).await.unwrap().unwrap();
    rows.condemn_source(&source).await;
    assert_eq!(
        rows.insert(&record, Some((&source, SourceTicket::Ledgered)))
            .await,
        Err(DocumentError::Unavailable)
    );
    assert!(rows.get(&record.metadata.id).await.unwrap().is_none());
}

#[test]
fn reservation_rejects_unbounded_or_untrusted_identifiers_and_sizes() {
    let valid =
        DocumentSourceReservation::new("owner", &format!("doc_{}", "a".repeat(32)), b"source");
    assert!(valid.validate().is_ok());
    for bytes in [0, MAX_DOCUMENT_BYTES as u64 + 1, u64::MAX] {
        assert!(DocumentSourceReservation {
            bytes,
            ..valid.clone()
        }
        .validate()
        .is_err());
    }
    assert!(DocumentSourceReservation {
        owner_hash: "../foreign".into(),
        ..valid.clone()
    }
    .validate()
    .is_err());
    assert!(DocumentSourceReservation {
        document_id: "doc_../foreign".into(),
        ..valid
    }
    .validate()
    .is_err());
}
