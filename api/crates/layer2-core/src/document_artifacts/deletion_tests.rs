use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::sync::Arc;

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
fn create() -> CreateDocument {
    CreateDocument {
        title: "Private report".into(),
        markdown: "# Private source".into(),
        origin_agent_id: None,
    }
}
fn fixture(
    enabled: bool,
) -> (
    Arc<DocumentService>,
    Arc<memory::MemoryDocuments>,
    Arc<memory::MemoryDocumentBlobs>,
) {
    let rows = Arc::new(memory::MemoryDocuments::default());
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    (
        DocumentService::with_deletion(rows.clone(), blobs.clone(), false, None, enabled),
        rows,
        blobs,
    )
}
fn deletion(revision: u64) -> DeleteDocument {
    DeleteDocument {
        expected_revision: revision,
    }
}

#[tokio::test]
async fn owner_delete_revokes_owner_share_link_and_comment_access_immediately() {
    let (service, rows, blobs) = fixture(true);
    let owner = caller("owner");
    let guest = caller("guest");
    let doc = service.create(&owner, "create", create()).await.unwrap();
    service
        .grant(
            &owner,
            &doc.id,
            "grant",
            "guest",
            GrantDocument {
                expected_revision: 1,
                permission: DocumentPermission::Comment,
            },
        )
        .await
        .unwrap();
    let token = "ab".repeat(32);
    service
        .create_link(
            &owner,
            &doc.id,
            "link",
            CreateDocumentLink {
                expected_revision: 2,
                permission: DocumentPermission::Comment,
                token: token.clone(),
            },
        )
        .await
        .unwrap();
    let result = service
        .delete(&owner, &doc.id, "delete", deletion(3))
        .await
        .unwrap();
    assert_eq!(result.revision, 4);
    assert_eq!(result.purge, DocumentPurgeState::Pending);
    for (actor, token) in [
        (&owner, None),
        (&guest, None),
        (&caller("link-reader"), Some(token.as_str())),
    ] {
        assert_eq!(
            service.read(actor, &doc.id, token).await,
            Err(DocumentError::NotFound)
        );
        assert_eq!(
            service.source(actor, &doc.id, token, None).await,
            Err(DocumentError::NotFound)
        );
        assert!(matches!(
            service.comments(actor, &doc.id, token, None, 25).await,
            Err(DocumentError::NotFound)
        ));
        assert!(matches!(
            service
                .comment(
                    actor,
                    &doc.id,
                    token,
                    "comment",
                    CreateDocumentComment {
                        body: "Late comment".into(),
                        document_revision: None
                    }
                )
                .await,
            Err(DocumentError::NotFound)
        ));
    }
    assert!(rows.get(&doc.id).await.unwrap().is_none());
    let tombstone = rows.tombstone(&doc.id).await.unwrap().unwrap();
    assert_eq!(tombstone.owner_hash, digest(b"owner"));
    assert_eq!(tombstone.objects.len(), 1);
    assert_eq!(
        blobs.0.lock().await.len(),
        1,
        "logical deletion retains objects until purge"
    );
    let serialized = serde_json::to_value(tombstone).unwrap();
    assert_eq!(serialized.as_object().unwrap().len(), 7);
    for secret in [
        "Private report",
        "Private source",
        "owner",
        "guest",
        "delete",
        token.as_str(),
    ] {
        assert!(!serialized.to_string().contains(&format!("\"{secret}\"")));
    }
}

#[tokio::test]
async fn non_owner_delete_is_not_found_and_changes_nothing() {
    let (service, rows, _) = fixture(true);
    let owner = caller("owner");
    let doc = service.create(&owner, "create", create()).await.unwrap();
    service
        .grant(
            &owner,
            &doc.id,
            "grant",
            "guest",
            GrantDocument {
                expected_revision: 1,
                permission: DocumentPermission::Comment,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .delete(&caller("guest"), &doc.id, "delete", deletion(2))
            .await,
        Err(DocumentError::NotFound)
    );
    assert!(rows.tombstone(&doc.id).await.unwrap().is_none());
    assert_eq!(
        service.read(&owner, &doc.id, None).await.unwrap().revision,
        2
    );
    service
        .delete(&owner, &doc.id, "delete", deletion(2))
        .await
        .unwrap();
    assert_eq!(
        service
            .delete(&caller("guest"), &doc.id, "delete", deletion(2))
            .await,
        Err(DocumentError::NotFound)
    );
}

#[tokio::test]
async fn delete_requires_expected_revision_and_conflicts_on_stale_revision() {
    assert!(serde_json::from_str::<DeleteDocument>("{}").is_err());
    let (service, _, _) = fixture(true);
    let owner = caller("owner");
    let doc = service.create(&owner, "create", create()).await.unwrap();
    assert_eq!(
        service.delete(&owner, &doc.id, "", deletion(1)).await,
        Err(DocumentError::Invalid)
    );
    assert_eq!(
        service.delete(&owner, &doc.id, "zero", deletion(0)).await,
        Err(DocumentError::Invalid)
    );
    assert_eq!(
        service.delete(&owner, &doc.id, "stale", deletion(2)).await,
        Err(DocumentError::Conflict)
    );
    assert!(service.read(&owner, &doc.id, None).await.is_ok());
}

#[tokio::test]
async fn exact_delete_retry_replays_original_outcome_and_different_body_conflicts() {
    let (service, rows, _) = fixture(true);
    let owner = caller("owner");
    let doc = service.create(&owner, "create", create()).await.unwrap();
    let first = service
        .delete(&owner, &doc.id, "delete", deletion(1))
        .await
        .unwrap();
    assert_eq!(
        service
            .delete(&owner, &doc.id, "delete", deletion(1))
            .await
            .unwrap(),
        first
    );
    rows.set_purge_state(&doc.id, DocumentPurgeState::Complete)
        .await;
    let retry = service
        .delete(&owner, &doc.id, "delete", deletion(1))
        .await
        .unwrap();
    assert_eq!(retry.deleted_at, first.deleted_at);
    assert_eq!(retry.revision, first.revision);
    assert_eq!(retry.purge, DocumentPurgeState::Complete);
    assert_eq!(
        service.delete(&owner, &doc.id, "delete", deletion(2)).await,
        Err(DocumentError::IdempotencyConflict)
    );
    assert_eq!(
        service
            .delete(&owner, &doc.id, "other-key", deletion(1))
            .await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service.create(&owner, "create", create()).await,
        Err(DocumentError::NotFound)
    );
}

#[tokio::test]
async fn deletion_flag_off_returns_typed_unavailable_and_never_writes_lifecycle() {
    let (service, rows, _) = fixture(false);
    let owner = caller("owner");
    let doc = service.create(&owner, "create", create()).await.unwrap();
    let before = rows.lifecycle_reads();
    assert_eq!(
        service.delete(&owner, &doc.id, "delete", deletion(1)).await,
        Err(DocumentError::DeletionUnavailable)
    );
    assert_eq!(rows.lifecycle_reads(), before);
    assert_eq!(service.usage(&owner).await.unwrap().pending_deletions, None);
    assert_eq!(rows.lifecycle_reads(), before);
    assert!(rows.tombstone(&doc.id).await.unwrap().is_none());
    assert!(service.read(&owner, &doc.id, None).await.is_ok());
}

#[tokio::test]
async fn list_and_search_exclude_deleted_document_immediately() {
    let (service, _, _) = fixture(true);
    let owner = caller("owner");
    let doc = service.create(&owner, "create", create()).await.unwrap();
    service
        .grant(
            &owner,
            &doc.id,
            "grant",
            "guest",
            GrantDocument {
                expected_revision: 1,
                permission: DocumentPermission::Read,
            },
        )
        .await
        .unwrap();
    service
        .delete(&owner, &doc.id, "delete", deletion(2))
        .await
        .unwrap();
    for actor in [&owner, &caller("guest")] {
        for q in [None, Some("private".into())] {
            assert!(service
                .list(
                    actor,
                    &DocumentQuery {
                        q,
                        ..Default::default()
                    }
                )
                .await
                .unwrap()
                .items
                .is_empty());
        }
    }
    assert_eq!(
        service.usage(&owner).await.unwrap().pending_deletions,
        Some(1)
    );
    assert_eq!(
        service
            .usage(&caller("guest"))
            .await
            .unwrap()
            .pending_deletions,
        Some(0)
    );
}

#[tokio::test]
async fn deletion_flag_off_process_cannot_recreate_tombstoned_id() {
    let (service, rows, blobs) = fixture(true);
    let owner = caller("owner");
    let doc = service.create(&owner, "create", create()).await.unwrap();
    let record = rows.get(&doc.id).await.unwrap().unwrap();
    service
        .delete(&owner, &doc.id, "delete", deletion(1))
        .await
        .unwrap();
    let old_process = DocumentService::new(rows.clone(), blobs);
    assert_eq!(
        old_process.create(&owner, "create", create()).await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(rows.insert(&record, None).await, Ok(InsertOutcome::Deleted));
}
