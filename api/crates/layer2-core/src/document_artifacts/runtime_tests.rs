use super::*;
use crate::{
    sandbox::{
        agent_access::AgentAccessError,
        agent_access_tool::{AgentAccessRequester, AgentAccessToolAuthenticator},
        AgentCreationOrigin, AgentCreationProvenance, AgentOrigin, RuntimeBinding,
    },
    AgentId, Authorization, Caller, Capability, SandboxId, SubjectId, WorkspaceId, WorktreeId,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

struct Authority(Mutex<Option<AgentAccessRequester>>);
#[async_trait::async_trait]
impl AgentAccessToolAuthenticator for Authority {
    async fn authenticate_setup(&self, _: &str) -> Result<(), AgentAccessError> {
        Err(AgentAccessError::Forbidden)
    }
    async fn authenticate(&self, _: &str) -> Result<AgentAccessRequester, AgentAccessError> {
        self.0
            .lock()
            .unwrap()
            .clone()
            .ok_or(AgentAccessError::Forbidden)
    }
}
struct RevocableUpload {
    blobs: memory::MemoryDocumentBlobs,
    authority: Arc<Authority>,
    revoke: AtomicBool,
}
#[async_trait::async_trait]
impl DocumentBlobStore for RevocableUpload {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        self.blobs.put(key, bytes).await?;
        if self.revoke.load(Ordering::SeqCst) {
            self.authority.0.lock().unwrap().take();
        }
        Ok(())
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, DocumentError> {
        self.blobs.get(key).await
    }
}
fn identity() -> AgentAccessRequester {
    let agent_id = AgentId("agt_00000000000000000000000000000001".into());
    let sandbox_id = SandboxId("sbx_document".into());
    let owner = SubjectId("document-owner".into());
    AgentAccessRequester {
        agent_id: agent_id.clone(),
        owner: owner.clone(),
        sandbox_id: sandbox_id.clone(),
        conversation: "native-one".into(),
        runtime: RuntimeBinding {
            runtime_id: "runtime-one".into(),
            runtime_version: "pinned-image".into(),
            provision_generation: Some(1),
            boot_id: Some("a".repeat(32)),
            bound_at: "2026-10-22T00:00:00Z".into(),
        },
        creation: AgentCreationProvenance {
            schema_version: 1,
            agent_id,
            agent_created_at: "2026-10-22T00:00:00Z".into(),
            sandbox_id,
            sandbox_created_at: "2026-10-22T00:00:00Z".into(),
            workspace_id: WorkspaceId("wks_document".into()),
            worktree_id: WorktreeId("wkt_document".into()),
            owner: crate::resource_access::ResourceOwner::Person { subject: owner },
            creation_origin: AgentCreationOrigin::Manual,
            created_by: None,
            provisioning_origin: AgentOrigin::Manual,
            creation_fingerprint: "c".repeat(64),
        },
    }
}
fn create() -> CreateDocument {
    CreateDocument {
        title: "Report".into(),
        markdown: "# Original".into(),
        origin_agent_id: Some("agt_00000000000000000000000000000002".into()),
    }
}
fn update() -> UpdateDocument {
    UpdateDocument {
        expected_revision: 1,
        title: None,
        markdown: Some("# Updated".into()),
    }
}
#[tokio::test]
async fn runtime_derives_owner_and_origin_and_cannot_adopt_an_owner_supplied_label() {
    let requester = identity();
    let auth = Authority(Mutex::new(Some(requester.clone())));
    let rows = Arc::new(memory::MemoryDocuments::default());
    let service = DocumentService::new(
        rows.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    );
    let runtime = service
        .create_for_agent(&auth, "fixture", "publish", create())
        .await
        .unwrap();
    assert_eq!(runtime.owner_subject, requester.owner.as_str());
    assert_eq!(
        runtime.origin_agent_id.as_deref(),
        Some(requester.agent_id.as_str())
    );
    assert_eq!(
        rows.get(&runtime.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_author_agent_id
            .as_deref(),
        Some(requester.agent_id.as_str())
    );
    let owner = Caller::new(
        requester.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let owned = service
        .create(
            &owner,
            "owner-label",
            CreateDocument {
                origin_agent_id: Some(requester.agent_id.to_string()),
                ..create()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .update_for_agent(&auth, "fixture", &owned.id, "change", update())
            .await,
        Err(DocumentError::NotFound)
    );
    service
        .update_for_agent(&auth, "fixture", &runtime.id, "change", update())
        .await
        .unwrap();
    auth.0.lock().unwrap().take();
    assert_eq!(
        service
            .update_for_agent(&auth, "fixture", &runtime.id, "change", update())
            .await,
        Err(DocumentError::NotFound)
    );
    assert_eq!(
        service
            .read(&owner, &runtime.id, None)
            .await
            .unwrap()
            .revision,
        2
    );
}
#[tokio::test]
async fn runtime_revocation_during_blob_upload_prevents_create_and_update_commits() {
    let auth = Arc::new(Authority(Mutex::new(Some(identity()))));
    let rows = Arc::new(memory::MemoryDocuments::default());
    let blobs = Arc::new(RevocableUpload {
        blobs: Default::default(),
        authority: auth.clone(),
        revoke: AtomicBool::new(true),
    });
    let service = DocumentService::new(rows.clone(), blobs.clone());
    assert_eq!(
        service
            .create_for_agent(auth.as_ref(), "fixture", "publish", create())
            .await,
        Err(DocumentError::NotFound)
    );
    assert!(rows.0.lock().await.is_empty());
    *auth.0.lock().unwrap() = Some(identity());
    blobs.revoke.store(false, Ordering::SeqCst);
    let doc = service
        .create_for_agent(auth.as_ref(), "fixture", "publish", create())
        .await
        .unwrap();
    blobs.revoke.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .update_for_agent(auth.as_ref(), "fixture", &doc.id, "update", update())
            .await,
        Err(DocumentError::NotFound)
    );
    let retained = rows.get(&doc.id).await.unwrap().unwrap();
    assert_eq!(retained.metadata.revision, 1);
    assert_eq!(retained.revisions.len(), 1);
}

struct TransientAuthority {
    requester: AgentAccessRequester,
    calls: std::sync::atomic::AtomicUsize,
    failing_at: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl AgentAccessToolAuthenticator for TransientAuthority {
    async fn authenticate_setup(&self, _: &str) -> Result<(), AgentAccessError> {
        Ok(())
    }
    async fn authenticate(&self, _: &str) -> Result<AgentAccessRequester, AgentAccessError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.failing_at.load(Ordering::SeqCst) {
            Err(AgentAccessError::Unavailable)
        } else {
            Ok(self.requester.clone())
        }
    }
}
#[tokio::test]
async fn transient_post_upload_authority_failure_can_retry_same_create_and_update_keys() {
    let auth = TransientAuthority {
        requester: identity(),
        calls: 0.into(),
        failing_at: 3.into(), // Initial, pre-reservation, then post-upload authority.
    };
    let rows = Arc::new(memory::MemoryDocuments::default());
    let service = DocumentService::new(
        rows.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    );
    assert_eq!(
        service
            .create_for_agent(&auth, "fixture", "same-key", create())
            .await,
        Err(DocumentError::Unavailable)
    );
    assert!(rows.0.lock().await.is_empty());
    let doc = service
        .create_for_agent(&auth, "fixture", "same-key", create())
        .await
        .unwrap();
    auth.failing_at
        .store(auth.calls.load(Ordering::SeqCst) + 3, Ordering::SeqCst);
    assert_eq!(
        service
            .update_for_agent(&auth, "fixture", &doc.id, "same-update", update())
            .await,
        Err(DocumentError::Unavailable)
    );
    assert_eq!(
        rows.get(&doc.id).await.unwrap().unwrap().metadata.revision,
        1
    );
    let changed = service
        .update_for_agent(&auth, "fixture", &doc.id, "same-update", update())
        .await
        .unwrap();
    assert_eq!(changed.revision, 2);
    assert_eq!(
        service
            .update_for_agent(&auth, "fixture", &doc.id, "same-update", update())
            .await
            .unwrap(),
        changed
    );
}

#[tokio::test]
async fn revoked_runtime_is_checked_before_reserving_document_storage() {
    struct QuotaCalls(std::sync::atomic::AtomicUsize);
    #[async_trait::async_trait]
    impl DocumentQuotaStore for QuotaCalls {
        async fn reserve(
            &self,
            _: &DocumentSourceReservation,
        ) -> Result<SourceTicket, DocumentError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(SourceTicket::Ledgered)
        }
    }
    let auth = TransientAuthority {
        requester: identity(),
        calls: 0.into(),
        failing_at: 2.into(),
    };
    let quota = Arc::new(QuotaCalls(0.into()));
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    let service = DocumentService::with_storage_quota(
        Arc::new(memory::MemoryDocuments::default()),
        blobs.clone(),
        false,
        Some(quota.clone()),
    );
    assert_eq!(
        service
            .create_for_agent(&auth, "fixture", "create", create())
            .await,
        Err(DocumentError::Unavailable)
    );
    assert_eq!(quota.0.load(Ordering::SeqCst), 0);
    assert!(blobs.0.lock().await.is_empty());
}
#[tokio::test]
async fn two_agents_can_publish_the_same_request_key_without_colliding() {
    let auth = Authority(Mutex::new(Some(identity())));
    let service = DocumentService::new(
        Arc::new(memory::MemoryDocuments::default()),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    );
    let first = service
        .create_for_agent(&auth, "fixture", "report", create())
        .await
        .unwrap();
    let mut second = identity();
    second.agent_id = AgentId("agt_00000000000000000000000000000002".into());
    second.creation.agent_id = second.agent_id.clone();
    *auth.0.lock().unwrap() = Some(second);
    let other = service
        .create_for_agent(&auth, "fixture", "report", create())
        .await
        .unwrap();
    assert_ne!(first.id, other.id);
    assert_eq!(first.owner_subject, other.owner_subject);
    assert_eq!(
        service
            .update_for_agent(&auth, "fixture", &first.id, "update", update())
            .await,
        Err(DocumentError::NotFound)
    );
}
