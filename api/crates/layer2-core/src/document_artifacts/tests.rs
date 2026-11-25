use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
fn body() -> CreateDocument {
    CreateDocument {
        title: "Research notes".into(),
        markdown: "# Persistent document\n\n- [x] retained".into(),
        origin_agent_id: Some("agt_00000000000000000000000000000001".into()),
    }
}
fn fixture() -> (Arc<DocumentService>, Arc<memory::MemoryDocuments>) {
    let store = Arc::new(memory::MemoryDocuments::default());
    (
        DocumentService::new(
            store.clone(),
            Arc::new(memory::MemoryDocumentBlobs::default()),
        )
        .with_directory(memory::recipient_directory(&["guest"])),
        store,
    )
}

#[tokio::test]
async fn independent_document_revision_and_idempotency_survive_without_an_agent() {
    let (s, rows) = fixture();
    let owner = caller("owner");
    let doc = s.create(&owner, "create", body()).await.unwrap();
    assert_eq!(s.create(&owner, "create", body()).await.unwrap().id, doc.id);
    assert_eq!(
        s.create(
            &owner,
            "create",
            CreateDocument {
                title: "different".into(),
                ..body()
            }
        )
        .await,
        Err(DocumentError::IdempotencyConflict)
    );
    let update = UpdateDocument {
        expected_revision: 1,
        title: Some("Renamed".into()),
        markdown: Some("# New source".into()),
    };
    let updated = s
        .update(&owner, &doc.id, "update", update.clone())
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(
        s.update(&owner, &doc.id, "update", update).await.unwrap(),
        updated
    );
    assert_eq!(
        s.update(
            &owner,
            &doc.id,
            "stale",
            UpdateDocument {
                expected_revision: 1,
                title: Some("Lost write".into()),
                markdown: None
            }
        )
        .await,
        Err(DocumentError::Conflict)
    );
    assert_eq!(
        s.source(&owner, &doc.id, None, Some(1))
            .await
            .unwrap()
            .markdown,
        body().markdown
    );
    assert_eq!(
        s.source(&owner, &doc.id, None, None)
            .await
            .unwrap()
            .markdown,
        "# New source"
    );
    rows.get(&doc.id)
        .await
        .unwrap()
        .unwrap()
        .validate()
        .unwrap();
}

#[tokio::test]
async fn ownership_search_permissions_and_comment_replay_are_isolated() {
    let (s, _) = fixture();
    let owner = caller("owner");
    let guest = caller("guest");
    let doc = s.create(&owner, "one", body()).await.unwrap();
    assert_eq!(
        s.read(&guest, &doc.id, None).await,
        Err(DocumentError::NotFound)
    );
    assert!(s
        .list(&guest, &DocumentQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
    let read = s
        .grant(
            &owner,
            &doc.id,
            "read",
            "guest",
            GrantDocument {
                expected_revision: 1,
                permission: DocumentPermission::Read,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        s.read(&guest, &doc.id, None).await.unwrap().access,
        DocumentAccess::Read
    );
    let comment = CreateDocumentComment {
        body: "Comment".into(),
        document_revision: None,
    };
    assert!(matches!(
        s.comment(&guest, &doc.id, None, "comment", comment.clone())
            .await,
        Err(DocumentError::NotFound)
    ));
    assert_eq!(
        s.update(
            &guest,
            &doc.id,
            "edit",
            UpdateDocument {
                expected_revision: read.revision,
                title: Some("Wrong".into()),
                markdown: None
            }
        )
        .await,
        Err(DocumentError::NotFound)
    );
    s.grant(
        &owner,
        &doc.id,
        "comment-grant",
        "guest",
        GrantDocument {
            expected_revision: 2,
            permission: DocumentPermission::Comment,
        },
    )
    .await
    .unwrap();
    let accepted = s
        .comment(&guest, &doc.id, None, "comment", comment.clone())
        .await
        .unwrap();
    assert_eq!(
        s.comment(&guest, &doc.id, None, "comment", comment)
            .await
            .unwrap()
            .comment,
        accepted.comment
    );
    assert_eq!(
        s.comments(&owner, &doc.id, None, None, 25)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let query = DocumentQuery {
        scope: Some("shared".into()),
        q: Some("RESEARCH".into()),
        ..Default::default()
    };
    assert_eq!(s.list(&guest, &query).await.unwrap().items.len(), 1);
    s.revoke(
        &owner,
        &doc.id,
        "revoke",
        "guest",
        accepted.document.revision,
    )
    .await
    .unwrap();
    assert_eq!(
        s.read(&guest, &doc.id, None).await,
        Err(DocumentError::NotFound)
    );
    assert!(s.list(&guest, &query).await.unwrap().items.is_empty());
}

#[tokio::test]
async fn links_require_authentication_secret_and_live_grant_and_never_enter_search() {
    let (s, rows) = fixture();
    let owner = caller("owner");
    let guest = caller("guest");
    let doc = s.create(&owner, "one", body()).await.unwrap();
    let token = "7e".repeat(32);
    let link = s
        .create_link(
            &owner,
            &doc.id,
            "link",
            CreateDocumentLink {
                expected_revision: 1,
                permission: DocumentPermission::Comment,
                token: token.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        s.read(&guest, &doc.id, Some(&token)).await.unwrap().access,
        DocumentAccess::Comment
    );
    assert!(s
        .list(&guest, &DocumentQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(
        !serde_json::to_string(&rows.get(&doc.id).await.unwrap().unwrap())
            .unwrap()
            .contains(&token)
    );
    let unauth = Caller::new(SubjectId::new("guest"), Authorization::none());
    assert_eq!(
        s.read(&unauth, &doc.id, Some(&token)).await,
        Err(DocumentError::NotFound)
    );
    s.revoke_link(&owner, &doc.id, "unlink", &link.link.unwrap().id, 2)
        .await
        .unwrap();
    assert_eq!(
        s.read(&guest, &doc.id, Some(&token)).await,
        Err(DocumentError::NotFound)
    );
}

struct HeldBlob {
    inner: memory::MemoryDocumentBlobs,
    hold: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl DocumentBlobStore for HeldBlob {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        self.inner.put(key, bytes).await
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, DocumentError> {
        let value = self.inner.get(key).await?;
        if self.hold.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(value)
    }
}
#[tokio::test]
async fn revoking_while_source_is_in_flight_prevents_delivery() {
    let blobs = Arc::new(HeldBlob {
        inner: Default::default(),
        hold: AtomicBool::new(false),
        entered: Default::default(),
        release: Default::default(),
    });
    let service = DocumentService::new(Arc::new(memory::MemoryDocuments::default()), blobs.clone())
        .with_directory(memory::recipient_directory(&["guest"]));
    let owner = caller("owner");
    let guest = caller("guest");
    let doc = service.create(&owner, "create", body()).await.unwrap();
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
    blobs.hold.store(true, Ordering::SeqCst);
    let read = tokio::spawn({
        let s = service.clone();
        let id = doc.id.clone();
        async move { s.source(&guest, &id, None, None).await }
    });
    blobs.entered.notified().await;
    service
        .revoke(&owner, &doc.id, "revoke", "guest", 2)
        .await
        .unwrap();
    blobs.release.notify_one();
    assert_eq!(read.await.unwrap(), Err(DocumentError::NotFound));
}

#[tokio::test]
async fn concurrent_title_changes_cannot_overwrite_each_other() {
    let (s, _) = fixture();
    let owner = caller("owner");
    let doc = s.create(&owner, "one", body()).await.unwrap();
    let a = UpdateDocument {
        expected_revision: 1,
        title: Some("A".into()),
        markdown: None,
    };
    let b = UpdateDocument {
        expected_revision: 1,
        title: Some("B".into()),
        markdown: None,
    };
    let (a, b) = tokio::join!(
        s.update(&owner, &doc.id, "a", a),
        s.update(&owner, &doc.id, "b", b)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(a == Err(DocumentError::Conflict) || b == Err(DocumentError::Conflict));
    assert_eq!(s.read(&owner, &doc.id, None).await.unwrap().revision, 2);
}

#[tokio::test]
async fn document_deletion_fences_owner_revision_and_replays_the_original_receipt() {
    let (service, store) = fixture();
    let owner = caller("owner");
    let guest = caller("guest");
    let doc = service.create(&owner, "create", body()).await.unwrap();
    assert_eq!(
        service.delete(&owner, &doc.id, "delete", 1).await,
        Err(DocumentError::Unavailable)
    );
    let service = service.with_deletion_enabled(true);
    for (revision, markdown) in [(1, "# Replacement".to_owned()), (2, body().markdown)] {
        service
            .update(
                &owner,
                &doc.id,
                &format!("edit-{revision}"),
                UpdateDocument {
                    expected_revision: revision,
                    title: None,
                    markdown: Some(markdown),
                },
            )
            .await
            .unwrap();
    }
    let shared = service
        .grant(
            &owner,
            &doc.id,
            "grant",
            "guest",
            GrantDocument {
                expected_revision: 3,
                permission: DocumentPermission::Read,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .delete(&guest, &doc.id, "delete", shared.revision)
            .await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service.delete(&owner, &doc.id, "delete", 1).await,
        Err(DocumentError::Conflict)
    );
    let receipt = service
        .delete(&owner, &doc.id, "delete", shared.revision)
        .await
        .unwrap();
    assert_eq!(receipt.revision, shared.revision + 1);
    assert_eq!(
        service
            .delete(&owner, &doc.id, "delete", shared.revision)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        service.delete(&owner, &doc.id, "delete", 1).await,
        Err(DocumentError::IdempotencyConflict)
    );
    assert_eq!(
        service
            .delete(&guest, &doc.id, "delete", shared.revision)
            .await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service.read(&owner, &doc.id, None).await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service.read(&guest, &doc.id, None).await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service.source(&owner, &doc.id, None, None).await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service.create(&owner, "create", body()).await,
        Err(DocumentError::NotFound)
    );
    assert!(store.get(&doc.id).await.unwrap().is_none());
    assert!(store.tombstone(&doc.id).await.unwrap().is_some());
}
