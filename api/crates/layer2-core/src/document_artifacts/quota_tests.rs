use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Mutex;

#[derive(Default)]
struct Quota {
    denied: AtomicBool,
    reservations: Mutex<Vec<DocumentSourceReservation>>,
}
#[async_trait::async_trait]
impl DocumentQuotaStore for Quota {
    async fn reserve(&self, source: &DocumentSourceReservation) -> Result<(), DocumentError> {
        source.validate()?;
        if self.denied.load(Ordering::SeqCst) {
            return Err(DocumentError::Capacity);
        }
        self.reservations.lock().await.push(source.clone());
        Ok(())
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
