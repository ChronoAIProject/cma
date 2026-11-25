//! Durable-document REST contract fixtures, with no Agent or Sandbox dependency.
use super::Scenario;
use layer2_core::{
    account_deletion::{AccountDeletionError, AccountStanding},
    document_artifacts::*,
    Authorization, Caller, Capability, SubjectId, UserDirectory,
};
use std::sync::{atomic::Ordering, Arc};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("document_")?.split("__").next().unwrap();
    let owner = Caller::new(
        SubjectId::new("document-owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let rows = Arc::new(memory::MemoryDocuments::default());
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    let recipients = memory::MemoryRecipientDirectory::new(&["document-reader", "Q"]);
    let directory = UserDirectory::new(recipients.clone());
    let service =
        DocumentService::new(rows.clone(), blobs.clone()).with_directory(directory.clone());
    let d = service
        .create(
            &owner,
            "fixture",
            CreateDocument {
                title: "Independent report".into(),
                markdown: "# Durable report\n".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    service
        .grant(
            &owner,
            &d.id,
            "grant",
            "document-reader",
            GrantDocument {
                expected_revision: 1,
                permission: DocumentPermission::Comment,
            },
        )
        .await
        .unwrap();
    service
        .create_link(
            &owner,
            &d.id,
            "link",
            CreateDocumentLink {
                expected_revision: 2,
                permission: DocumentPermission::Read,
                token: "01".repeat(32),
            },
        )
        .await
        .unwrap();
    service
        .comment(
            &owner,
            &d.id,
            None,
            "comment",
            CreateDocumentComment {
                body: "Recorded comment".into(),
                document_revision: None,
            },
        )
        .await
        .unwrap();
    // Fixed seed timestamps are deliberate fixture data; production commands
    // still supply their real timestamp and the corpus captures that scalar.
    let mut records = rows.0.lock().await;
    for r in records.values_mut() {
        r.metadata.created_at = "2026-10-22T00:00:00Z".into();
        r.metadata.updated_at = "2026-10-22T00:00:00Z".into();
        for revision in &mut r.revisions {
            revision.metadata.created_at = "2026-10-22T00:00:00Z".into();
        }
        for link in &mut r.links {
            link.metadata.created_at = "2026-10-22T00:00:00Z".into();
        }
        for comment in &mut r.comments {
            comment.created_at = "2026-10-22T00:00:00Z".into();
        }
    }
    drop(records);
    let service = if matches!(mode, "unavailable" | "capacity") {
        DocumentService::new(
            Arc::new(Faults {
                rows,
                mode: mode.into(),
            }),
            blobs,
        )
        .with_directory(directory)
    } else {
        service
    };
    if mode == "directory-outage" {
        recipients.unavailable.store(true, Ordering::SeqCst);
    }
    let service = match mode {
        "recipient_erasing" => {
            service.with_account_standing(Arc::new(super::account_deletion_cases::ErasingRecipient))
        }
        "owner-erasing" => service.with_account_standing(Arc::new(ErasingDocumentOwner)),
        _ => service,
    };
    let service =
        service.with_deletion_enabled(name.ends_with("__delete") && mode != "deletion-disabled");
    Some(Scenario {
        router: super::super::documents_router(service),
        subject: match mode {
            "anonymous" => None,
            "foreign" | "link" => Some("document-stranger"),
            "reader" => Some("document-reader"),
            _ => Some("document-owner"),
        },
        authorization: if mode == "forbidden" {
            Authorization::none()
        } else {
            owner.authorization
        },
        symbols: Default::default(),
    })
}
struct ErasingDocumentOwner;
#[async_trait::async_trait]
impl AccountStanding for ErasingDocumentOwner {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(subject.as_str() == "document-owner")
    }
}

struct Faults {
    rows: Arc<memory::MemoryDocuments>,
    mode: String,
}
#[async_trait::async_trait]
impl DocumentStore for Faults {
    async fn get(&self, id: &str) -> Result<Option<StoredDocument>, DocumentError> {
        if self.mode == "unavailable" {
            Err(DocumentError::Unavailable)
        } else {
            self.rows.get(id).await
        }
    }
    async fn delete(&self, _: &str, _: &DocumentTombstone) -> Result<bool, DocumentError> {
        Err(DocumentError::Capacity)
    }
    async fn list(
        &self,
        _: &str,
        _: &DocumentQuery,
    ) -> Result<Vec<DocumentListing>, DocumentError> {
        Err(DocumentError::Unavailable)
    }
    async fn insert(&self, _: &StoredDocument) -> Result<bool, DocumentError> {
        Err(DocumentError::Unavailable)
    }
    async fn compare_exchange(&self, _: u64, _: &StoredDocument) -> Result<bool, DocumentError> {
        Err(DocumentError::Capacity)
    }
}
