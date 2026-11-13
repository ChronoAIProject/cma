use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::sync::{atomic::Ordering, Arc};

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

fn create() -> CreateDocument {
    CreateDocument {
        title: "Quota report".into(),
        markdown: "body".into(),
        origin_agent_id: None,
    }
}

#[tokio::test]
async fn quota_refusal_reports_dimension_limit_and_live_without_retry_after() {
    for (limits, dimension, limit, live) in [
        ([1, 4, 1], DocumentLimit::Documents, 1, 1),
        ([2, 6, 1], DocumentLimit::Bytes, 6, 4),
        ([2, 8, 1], DocumentLimit::Objects, 1, 1),
    ] {
        let quota = Arc::new(memory::MemoryDocumentQuota::new(limits));
        let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
        let service = DocumentService::with_storage_quota(
            Arc::new(memory::MemoryDocuments::default()),
            blobs.clone(),
            false,
            Some(quota),
        );
        service
            .create(&caller("owner"), "first", create())
            .await
            .unwrap();
        assert_eq!(
            service.create(&caller("owner"), "refused", create()).await,
            Err(DocumentError::LimitReached {
                dimension,
                limit,
                live
            })
        );
        assert_eq!(blobs.0.lock().await.len(), 1);
        assert_eq!(
            service
                .usage(&caller("owner"))
                .await
                .unwrap()
                .bytes
                .unwrap()
                .used,
            4
        );
    }
}

#[tokio::test]
async fn usage_is_owner_scoped_and_reports_not_enforced_without_ready_ledger() {
    let quota = Arc::new(memory::MemoryDocumentQuota::new([100, 1024, 128]));
    let rows = Arc::new(memory::MemoryDocuments::default());
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    let service = DocumentService::with_storage_quota(
        rows.clone(),
        blobs.clone(),
        false,
        Some(quota.clone()),
    );
    let owner = caller("owner");
    let other = caller("other-owner");
    let document = service.create(&owner, "first", create()).await.unwrap();
    service
        .grant(
            &owner,
            &document.id,
            "share",
            "other-owner",
            GrantDocument {
                expected_revision: 1,
                permission: DocumentPermission::Read,
            },
        )
        .await
        .unwrap();
    let usage = service.usage(&owner).await.unwrap();
    assert_eq!(usage.enforcement, QuotaEnforcement::Enforced);
    assert_eq!(
        usage.documents,
        Some(UsageCounter {
            used: 1,
            limit: 100
        })
    );
    assert_eq!(
        usage.bytes,
        Some(UsageCounter {
            used: 4,
            limit: 1024
        })
    );
    assert_eq!(
        usage.objects,
        Some(UsageCounter {
            used: 1,
            limit: 128
        })
    );
    let foreign = service.usage(&other).await.unwrap();
    assert_eq!(foreign.documents.unwrap().used, 0);
    assert_eq!(foreign.bytes.unwrap().used, 0);
    assert_eq!(foreign.objects.unwrap().used, 0);
    assert!(service.read(&other, &document.id, None).await.is_ok());

    quota.ready.store(false, Ordering::SeqCst);
    assert_eq!(
        service.usage(&owner).await,
        Ok(DocumentUsage::not_enforced())
    );
    let unconfigured = DocumentService::new(rows, blobs);
    assert_eq!(
        unconfigured.usage(&owner).await,
        Ok(DocumentUsage::not_enforced())
    );
    let forbidden = Caller::new(SubjectId::new("owner"), Authorization::none());
    assert_eq!(
        unconfigured.usage(&forbidden).await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service.usage(&forbidden).await,
        Err(DocumentError::NotFound)
    );
    quota.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(service.usage(&owner).await, Err(DocumentError::Unavailable));
}
