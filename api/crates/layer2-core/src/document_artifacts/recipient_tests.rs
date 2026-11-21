use super::*;
use crate::{
    account_deletion::{AccountDeletionError, AccountStanding},
    Authorization, Caller, Capability, SubjectId, UserDirectory,
};
use std::sync::{atomic::Ordering, Arc};

struct DeletedRecipient;
#[async_trait::async_trait]
impl AccountStanding for DeletedRecipient {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(subject.as_str() == "Recipient-Deleted")
    }
}

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

async fn fixture() -> (
    Arc<DocumentService>,
    Arc<memory::MemoryDocuments>,
    Arc<memory::MemoryRecipientDirectory>,
    DocumentView,
) {
    let rows = Arc::new(memory::MemoryDocuments::default());
    let directory =
        memory::MemoryRecipientDirectory::new(&["owner", "Recipient-A", "Recipient-Deleted"]);
    let service = DocumentService::new(
        rows.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    )
    .with_account_standing(Arc::new(DeletedRecipient))
    .with_directory(UserDirectory::new(directory.clone()));
    let document = service
        .create(
            &caller("owner"),
            "create",
            CreateDocument {
                title: "Notes".into(),
                markdown: "# Notes".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    (service, rows, directory, document)
}

fn command() -> GrantDocument {
    GrantDocument {
        expected_revision: 1,
        permission: DocumentPermission::Read,
    }
}

#[tokio::test]
async fn owner_selects_an_exact_registered_recipient_and_replays_the_grant_during_outage() {
    let (service, rows, directory, document) = fixture().await;
    let owner = caller("owner");
    assert_eq!(
        service
            .share_recipients(&owner, &document.id, "Recipient")
            .await
            .unwrap(),
        vec![DocumentShareRecipient {
            subject: SubjectId::new("Recipient-A"),
            display_name: Some("Name Recipient-A".into()),
        }],
    );
    assert!(service
        .share_recipients(&owner, &document.id, "owner")
        .await
        .unwrap()
        .is_empty());
    assert!(service
        .share_recipients(&owner, &document.id, "R")
        .await
        .unwrap()
        .is_empty());
    let granted = service
        .grant(&owner, &document.id, "grant", "Recipient-A", command())
        .await
        .unwrap();
    directory.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .grant(&owner, &document.id, "grant", "Recipient-A", command())
            .await,
        Ok(granted.clone()),
    );
    assert_eq!(
        service
            .share_recipients(&caller("Recipient-A"), &document.id, "Recipient")
            .await,
        Err(DocumentError::NotFound),
    );
    let stored = rows.get(&document.id).await.unwrap().unwrap();
    assert_eq!(stored.metadata.revision, 2);
    assert_eq!(stored.metadata, granted);
    assert_eq!(stored.receipts.len(), 1);
    assert_eq!(stored.shares.len(), 1);
    assert_eq!(stored.shares[0].subject, "Recipient-A");
    assert_eq!(
        service
            .list(&caller("Recipient-A"), &DocumentQuery::default())
            .await
            .unwrap()
            .items
            .len(),
        1,
    );
}

#[tokio::test]
async fn refused_recipients_leave_the_aggregate_and_inventory_unchanged() {
    let (service, rows, directory, document) = fixture().await;
    let owner = caller("owner");
    let before = serde_json::to_value(rows.get(&document.id).await.unwrap()).unwrap();
    for (subject, expected) in [
        (" Recipient-A", DocumentError::InvalidShareRecipient),
        ("owner", DocumentError::InvalidShareRecipient),
        ("Unregistered", DocumentError::ShareRecipientUnavailable),
        ("recipient-a", DocumentError::ShareRecipientUnavailable),
        (
            "Recipient-Deleted",
            DocumentError::ShareRecipientUnavailable,
        ),
    ] {
        assert_eq!(
            service
                .grant(&owner, &document.id, "grant", subject, command())
                .await,
            Err(expected),
        );
        assert_eq!(
            serde_json::to_value(rows.get(&document.id).await.unwrap()).unwrap(),
            before
        );
        let listing = service
            .list(
                &caller(subject.trim()),
                &DocumentQuery {
                    scope: Some("shared".into()),
                    ..Default::default()
                },
            )
            .await;
        if subject == "Recipient-Deleted" {
            assert!(matches!(
                listing,
                Err(DocumentError::AccountDeletionInProgress)
            ));
        } else {
            assert!(listing.unwrap().items.is_empty());
        }
    }
    directory.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .grant(&owner, &document.id, "grant", "Recipient-A", command())
            .await,
        Err(DocumentError::RecipientDirectoryUnavailable),
    );
    assert_eq!(
        service
            .grant(
                &caller("unrelated"),
                &document.id,
                "grant",
                "Recipient-A",
                command()
            )
            .await,
        Err(DocumentError::NotFound),
    );
    assert_eq!(
        serde_json::to_value(rows.get(&document.id).await.unwrap()).unwrap(),
        before
    );
    assert!(service
        .shares(&owner, &document.id)
        .await
        .unwrap()
        .is_empty());
    directory.unavailable.store(false, Ordering::SeqCst);
    assert!(service
        .grant(&owner, &document.id, "grant", "Recipient-A", command())
        .await
        .is_ok());
}
