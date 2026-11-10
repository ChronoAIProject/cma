use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::sync::Arc;

pub(super) fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

pub(super) async fn fixture() -> (Arc<DocumentService>, Arc<memory::MemoryDocuments>, String) {
    let rows = Arc::new(memory::MemoryDocuments::default());
    let service = DocumentService::with_revocation_reserve(
        rows.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
        true,
    );
    let doc = service
        .create(
            &caller("owner"),
            "create",
            CreateDocument {
                title: "Retained notes".into(),
                markdown: "Private source".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    service
        .grant(
            &caller("owner"),
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
    (service, rows, doc.id)
}

pub(super) async fn fill_receipts(rows: &memory::MemoryDocuments, id: &str) -> u64 {
    let mut rows = rows.0.lock().await;
    let record = rows.get_mut(id).unwrap();
    let receipt = record.receipts.values().next().unwrap().clone();
    for n in record.receipts.len()..MAX_RECEIPTS {
        record.receipts.insert(format!("{n:064x}"), receipt.clone());
    }
    record.metadata.revision = MAX_RECEIPTS as u64 + 1;
    record.validate().unwrap();
    record.metadata.revision
}

#[tokio::test]
async fn full_receipts_allow_revoke_and_exact_retry_without_resurrecting_grants() {
    let (service, rows, id) = fixture().await;
    let revision = fill_receipts(&rows, &id).await;
    let owner = caller("owner");
    let guest = caller("guest");
    assert!(service.read(&guest, &id, None).await.is_ok());
    assert_eq!(
        service
            .revoke(&guest, &id, "steal", "guest", revision)
            .await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service.revoke(&owner, &id, "stale", "guest", 1).await,
        Err(DocumentError::Conflict)
    );
    assert!(matches!(
        service
            .comment(
                &guest,
                &id,
                None,
                "full",
                CreateDocumentComment {
                    body: "New comment".into(),
                    document_revision: None,
                },
            )
            .await,
        Err(DocumentError::Capacity)
    ));
    let revoked = service
        .revoke(&owner, &id, "revoke", "guest", revision)
        .await
        .unwrap();
    assert_eq!(
        service
            .revoke(&owner, &id, "revoke", "guest", revision)
            .await,
        Ok(revoked)
    );
    assert_eq!(
        service
            .revoke(&owner, &id, "revoke", "other", revision)
            .await,
        Err(DocumentError::IdempotencyConflict)
    );
    // The old successful receipt is replayed without applying its old policy.
    assert!(service
        .grant(
            &owner,
            &id,
            "grant",
            "guest",
            GrantDocument {
                expected_revision: 1,
                permission: DocumentPermission::Comment,
            },
        )
        .await
        .is_ok());
    assert_eq!(
        service.read(&guest, &id, None).await,
        Err(DocumentError::NotFound)
    );
    let record = rows.get(&id).await.unwrap().unwrap();
    assert_eq!(record.receipts.len(), MAX_RECEIPTS + 1);
    record.validate().unwrap();
}

#[tokio::test]
async fn every_live_grant_and_link_can_be_revoked_after_receipts_fill() {
    let (service, rows, id) = fixture().await;
    let token = "ab".repeat(32);
    let link = service
        .create_link(
            &caller("owner"),
            &id,
            "link",
            CreateDocumentLink {
                expected_revision: 2,
                permission: DocumentPermission::Read,
                token: token.clone(),
            },
        )
        .await
        .unwrap()
        .link
        .unwrap();
    {
        let mut records = rows.0.lock().await;
        let record = records.get_mut(&id).unwrap();
        for n in 1..MAX_GRANTS {
            record.shares.push(DocumentShare {
                subject: format!("guest-{n}"),
                permission: DocumentPermission::Read,
            });
        }
        for n in 1..MAX_LINKS {
            record.links.push(StoredLink {
                metadata: DocumentLink {
                    id: types::identity("dln_", &[&n.to_string()]),
                    permission: DocumentPermission::Read,
                    created_at: crate::now_rfc3339(),
                },
                token_sha256: digest(format!("fixture-{n}").as_bytes()),
            });
        }
    }
    let mut revision = fill_receipts(&rows, &id).await;
    let record = rows.get(&id).await.unwrap().unwrap();
    assert!(service
        .read(&caller("reader"), &id, Some(&token))
        .await
        .is_ok());
    assert_eq!(
        service
            .revoke_link(&caller("guest"), &id, "deny", &link.id, revision)
            .await,
        Err(DocumentError::NotFound)
    );
    for share in record.shares {
        revision = service
            .revoke(
                &caller("owner"),
                &id,
                &share.subject,
                &share.subject,
                revision,
            )
            .await
            .unwrap()
            .revision;
    }
    for link in record.links {
        let prior = revision;
        let result = service
            .revoke_link(
                &caller("owner"),
                &id,
                &link.metadata.id,
                &link.metadata.id,
                prior,
            )
            .await
            .unwrap();
        assert_eq!(
            service
                .revoke_link(
                    &caller("owner"),
                    &id,
                    &link.metadata.id,
                    &link.metadata.id,
                    prior
                )
                .await,
            Ok(result.clone())
        );
        revision = result.revision;
    }
    assert_eq!(
        service.read(&caller("reader"), &id, Some(&token)).await,
        Err(DocumentError::NotFound)
    );
    let record = rows.get(&id).await.unwrap().unwrap();
    assert_eq!(record.receipts.len(), MAX_RECEIPTS + MAX_GRANTS + MAX_LINKS);
    assert!(record.shares.is_empty() && record.links.is_empty());
    record.validate().unwrap();
    assert_eq!(
        service
            .revoke(&caller("owner"), &id, "absent", "guest", revision)
            .await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        rows.get(&id).await.unwrap().unwrap().receipts.len(),
        record.receipts.len()
    );
}

#[tokio::test]
async fn metadata_saturation_preserves_revoke_headroom_and_denies_normal_growth() {
    let (service, rows, id) = fixture().await;
    let revision = fill_receipts(&rows, &id).await;
    fill_metadata(&rows, &id).await;
    // Count still has room: the independent metadata budget must stop growth.
    assert_eq!(
        service
            .grant(
                &caller("owner"),
                &id,
                "metadata-full",
                "new-reader",
                GrantDocument {
                    expected_revision: revision,
                    permission: DocumentPermission::Read,
                },
            )
            .await,
        Err(DocumentError::Capacity)
    );
    service
        .revoke(&caller("owner"), &id, "revoke", "guest", revision)
        .await
        .unwrap();
    let record = rows.get(&id).await.unwrap().unwrap();
    assert!(record.metadata_bytes().unwrap() > MAX_METADATA_BYTES);
    record.validate().unwrap();
    assert_eq!(
        service.read(&caller("guest"), &id, None).await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service
            .grant(
                &caller("owner"),
                &id,
                "new-grant",
                "guest",
                GrantDocument {
                    expected_revision: revision + 1,
                    permission: DocumentPermission::Read,
                }
            )
            .await,
        Err(DocumentError::Capacity)
    );
}

#[tokio::test]
async fn absent_removals_do_not_spend_receipts_or_advance_revision() {
    let (service, rows, id) = fixture().await;
    let before = rows.get(&id).await.unwrap().unwrap();
    assert_eq!(
        service
            .remove_comment(
                &caller("guest"),
                &id,
                None,
                "missing",
                &types::identity("dcm_", &["absent"]),
                2
            )
            .await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service
            .revoke(&caller("owner"), &id, "missing", "other", 2)
            .await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service
            .revoke_link(
                &caller("owner"),
                &id,
                "missing",
                &types::identity("dln_", &["absent"]),
                2
            )
            .await,
        Err(DocumentError::NotFound)
    );
    let after = rows.get(&id).await.unwrap().unwrap();
    assert_eq!(before.metadata, after.metadata);
    assert_eq!(before.receipts.len(), after.receipts.len());
}

pub(super) async fn fill_metadata(rows: &memory::MemoryDocuments, id: &str) {
    {
        let mut records = rows.0.lock().await;
        let record = records.get_mut(id).unwrap();
        // Each historical comment fits its 8 KiB bound; escaping consumes
        // aggregate bytes. Receipts may retain comments removed long ago.
        record
            .receipts
            .remove(&format!("{:064x}", MAX_RECEIPTS - 1));
        for receipt in record.receipts.values_mut() {
            receipt.outcome.comment = Some(DocumentComment {
                id: types::identity("dcm_", &["historical"]),
                author_subject: "guest".into(),
                body: "\n".repeat(8191) + "x",
                document_revision: 1,
                created_at: crate::now_rfc3339(),
            });
        }
        let mut excess = record.metadata_bytes().unwrap() - MAX_METADATA_BYTES;
        for receipt in record.receipts.values_mut() {
            let body = &mut receipt.outcome.comment.as_mut().unwrap().body;
            let remove = (excess / 2).min(body.len() - 1);
            body.drain(..remove);
            excess -= remove * 2;
        }
        // Leave at most one byte below the ordinary metadata ceiling.
        if excess != 0 {
            let body = &mut record
                .receipts
                .values_mut()
                .find(|r| r.outcome.comment.as_ref().unwrap().body.len() > 1)
                .unwrap()
                .outcome
                .comment
                .as_mut()
                .unwrap()
                .body;
            body.replace_range(..1, "x");
        }
        assert_eq!(record.metadata_bytes().unwrap(), MAX_METADATA_BYTES);
        record.validate().unwrap();
    }
}
