//! Durable-document REST contract fixtures, with no Agent or Sandbox dependency.
use super::Scenario;
use layer2_core::{document_artifacts::*, Authorization, Caller, Capability, SubjectId};
use std::sync::Arc;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("document_")?.split("__").next().unwrap();
    let owner = Caller::new(
        SubjectId::new("document-owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let rows = Arc::new(memory::MemoryDocuments::default());
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    let service = DocumentService::new(rows.clone(), blobs.clone());
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
    let service = if mode.starts_with("delete_") {
        let store: Arc<dyn DocumentStore> = if mode == "delete_unavailable" {
            Arc::new(Faults {
                rows: rows.clone(),
                mode: "unavailable".into(),
            })
        } else {
            rows.clone()
        };
        let service =
            DocumentService::with_deletion(store, blobs, false, None, mode != "delete_off");
        if matches!(
            mode,
            "delete_retry"
                | "delete_changed"
                | "delete_blocked"
                | "delete_complete"
                | "delete_pending"
        ) {
            service
                .delete(
                    &owner,
                    &d.id,
                    "contract-delete",
                    DeleteDocument {
                        expected_revision: 4,
                    },
                )
                .await
                .unwrap();
            if mode == "delete_blocked" {
                rows.set_purge_state(&d.id, DocumentPurgeState::Blocked)
                    .await;
            } else if mode == "delete_complete" {
                rows.set_purge_state(&d.id, DocumentPurgeState::Complete)
                    .await;
            }
        }
        service
    } else if mode.starts_with("limit_") || mode.starts_with("usage_") {
        let limits = match mode {
            "limit_documents" => [1, 1024, 128],
            "limit_bytes" => [100, 17, 128],
            "limit_objects" => [100, 1024, 1],
            _ => [
                OWNER_DOCUMENT_LIMIT,
                OWNER_DOCUMENT_BYTES,
                OWNER_DOCUMENT_OBJECT_LIMIT,
            ],
        };
        let mut quota = memory::MemoryDocumentQuota::new(limits);
        quota
            .reserve(&DocumentSourceReservation {
                owner_hash: digest(b"document-owner"),
                document_id: d.id.clone(),
                sha256: d.sha256.clone(),
                bytes: d.size_bytes,
            })
            .await
            .unwrap();
        if mode == "limit_documents" {
            // Retained baselines can exceed a newly enforced limit.
            quota.limits[0] = 0;
        }
        quota.ready.store(
            mode != "usage_not_ready",
            std::sync::atomic::Ordering::SeqCst,
        );
        quota.unavailable.store(
            mode == "usage_unavailable",
            std::sync::atomic::Ordering::SeqCst,
        );
        DocumentService::with_storage_quota(rows, blobs, false, Some(Arc::new(quota)))
    } else if matches!(mode, "unavailable" | "capacity") {
        DocumentService::new(
            Arc::new(Faults {
                rows,
                mode: mode.into(),
            }),
            blobs,
        )
    } else {
        service
    };
    Some(Scenario {
        router: super::super::documents_router(service),
        subject: match mode {
            "anonymous" | "usage_anonymous" | "delete_anonymous" => None,
            "usage_foreign" => Some("document-stranger"),
            "foreign" | "link" | "delete_foreign" => Some("document-stranger"),
            "reader" => Some("document-reader"),
            _ => Some("document-owner"),
        },
        authorization: if matches!(mode, "forbidden" | "usage_forbidden" | "delete_forbidden") {
            Authorization::none()
        } else {
            owner.authorization
        },
        symbols: Default::default(),
    })
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
    async fn list(
        &self,
        _: &str,
        _: &DocumentQuery,
    ) -> Result<Vec<DocumentListing>, DocumentError> {
        Err(DocumentError::Unavailable)
    }
    async fn insert(
        &self,
        _: &StoredDocument,
        _: Option<(&DocumentSourceReservation, SourceTicket)>,
    ) -> Result<InsertOutcome, DocumentError> {
        Err(DocumentError::Unavailable)
    }
    async fn delete(&self, _: u64, _: &DocumentTombstone) -> Result<DeleteOutcome, DocumentError> {
        Err(DocumentError::Unavailable)
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
    async fn compare_exchange(
        &self,
        _: u64,
        _: &StoredDocument,
        _: Option<(&DocumentSourceReservation, SourceTicket)>,
    ) -> Result<bool, DocumentError> {
        Err(DocumentError::Capacity)
    }
}
