use super::*;
use crate::{
    account_deletion::{AccountDeletionError, AccountStanding},
    Authorization, Caller, Capability, SubjectId,
};
use std::{
    collections::BTreeSet,
    sync::{Arc, RwLock},
};
#[derive(Default)]
struct Standing(RwLock<BTreeSet<String>>);
#[async_trait::async_trait]
impl AccountStanding for Standing {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(self.0.read().unwrap().contains(subject.as_str()))
    }
}
fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
#[tokio::test]
async fn standing_gate_denies_document_owner_share_and_link_reads() {
    let standing = Arc::new(Standing::default());
    let service = DocumentService::new(
        Arc::new(memory::MemoryDocuments::default()),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    )
    .with_account_standing(standing.clone());
    let owner = caller("owner");
    let reader = caller("reader");
    let doc = service
        .create(
            &owner,
            "create",
            CreateDocument {
                title: "Title".into(),
                markdown: "# Source".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    let doc = service
        .grant(
            &owner,
            &doc.id,
            "grant",
            "reader",
            GrantDocument {
                expected_revision: 1,
                permission: DocumentPermission::Read,
            },
        )
        .await
        .unwrap();
    let token = "a".repeat(64);
    service
        .create_link(
            &owner,
            &doc.id,
            "link",
            CreateDocumentLink {
                expected_revision: doc.revision,
                permission: DocumentPermission::Read,
                token: token.clone(),
            },
        )
        .await
        .unwrap();
    standing.0.write().unwrap().insert("owner".into());
    for (actor, token) in [
        (&owner, None),
        (&reader, None),
        (&caller("linked"), Some(token.as_str())),
    ] {
        assert_eq!(
            service.read(actor, &doc.id, token).await,
            Err(DocumentError::AccountDeletionInProgress)
        );
        assert_eq!(
            service.source(actor, &doc.id, token, None).await,
            Err(DocumentError::AccountDeletionInProgress)
        );
    }
    assert!(service
        .list(&reader, &DocumentQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
}
#[tokio::test]
async fn share_grant_to_erasing_recipient_is_refused() {
    let standing = Arc::new(Standing::default());
    let service = DocumentService::new(
        Arc::new(memory::MemoryDocuments::default()),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    )
    .with_account_standing(standing.clone());
    let owner = caller("owner");
    let doc = service
        .create(
            &owner,
            "create",
            CreateDocument {
                title: "Title".into(),
                markdown: "# Source".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    standing.0.write().unwrap().insert("recipient".into());
    assert_eq!(
        service
            .grant(
                &owner,
                &doc.id,
                "grant",
                "recipient",
                GrantDocument {
                    expected_revision: 1,
                    permission: DocumentPermission::Read
                }
            )
            .await,
        Err(DocumentError::ShareRecipientUnavailable)
    );
    assert!(service.shares(&owner, &doc.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn deleting_document_owner_remains_concealed_from_unrelated_callers() {
    let standing = Arc::new(Standing::default());
    let service = DocumentService::new(
        Arc::new(memory::MemoryDocuments::default()),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    )
    .with_account_standing(standing.clone());
    let document = service
        .create(
            &caller("owner"),
            "create",
            CreateDocument {
                title: "Title".into(),
                markdown: "# Source".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    standing.0.write().unwrap().insert("owner".into());
    let invalid_token = "f".repeat(64);
    for token in [None, Some(invalid_token.as_str())] {
        assert_eq!(
            service
                .read(&caller("unrelated"), &document.id, token)
                .await,
            Err(DocumentError::NotFound)
        );
    }
}
