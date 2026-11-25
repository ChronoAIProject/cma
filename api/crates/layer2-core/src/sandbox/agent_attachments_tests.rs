use super::*;
use crate::sandbox::agent_attachments::*;
use crate::{resource_access::*, Authorization, Caller, Capability};
use layer3_adapter::*;
#[path = "agent_attachments_test_runtime.rs"]
pub(super) mod runtime;
use runtime::UnusedRuntime;
struct NoOrganizations;
#[async_trait::async_trait]
impl OrganizationAuthority for NoOrganizations {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> std::result::Result<CallerContext, MembershipError> {
        panic!("unexpected organization lookup")
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> std::result::Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("unexpected organization lookup")
    }
}
async fn fixture() -> (
    Arc<store::InMemorySandboxStore>,
    Arc<AgentAttachmentService>,
    Caller,
) {
    let store = Arc::new(store::InMemorySandboxStore::default());
    let mut root = sandbox();
    root.status = SandboxStatus::Active;
    store.create_sandbox(root).await.unwrap();
    store.create_workspace(workspace()).await.unwrap();
    store.create_worktree(worktree()).await.unwrap();
    let mut a = agent();
    a.status = AgentStatus::Ready;
    a.conversation.conversation_id = Some("conversation".into());
    a.conversation.runtime_generation = Some(1);
    a.work.step = AgentWorkStep::None;
    store.create_agent(a).await.unwrap();
    let service = AgentAttachmentService::new(
        Arc::new(ResourceAccessService::new(
            store.clone(),
            Arc::new(NoOrganizations),
        )),
        Arc::new(UnusedRuntime {
            status: None,
            file_root: None,
        }),
    );
    (
        store,
        service,
        Caller::new(
            SubjectId("owner".into()),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
    )
}
fn meta() -> AttachmentMetadata<'static> {
    AttachmentMetadata {
        key: "upload-one",
        name: "review.txt",
        media_type: "text/plain",
    }
}
#[test]
fn attachment_names_reject_traversal_and_media_remains_inert_metadata() {
    for value in ["../secret", "a/b", "a\\b", ".", "..", "", "line\nfeed"] {
        assert!(sanitize_name(value).is_err());
    }
    assert_eq!(sanitize_name("评审 report.png").unwrap(), "___report.png");
    assert_eq!(sanitize_name(".env").unwrap(), "_env");
    assert!(valid_media_type("application/x-custom+binary"));
    for value in [
        "text/plain; charset=utf-8",
        "invalid",
        "text/",
        "a/b/c",
        "image/png\n",
    ] {
        assert!(!valid_media_type(value));
    }
}
#[tokio::test]
async fn attachment_upload_refuses_cross_owner_and_legacy_home_before_runtime_effect() {
    let (_, service, owner) = fixture().await;
    let other = Caller::new(SubjectId("foreign".into()), owner.authorization.clone());
    assert!(matches!(
        service
            .admit(&other, None, None, &AgentId("agt_test".into()), meta())
            .await,
        Err(AttachmentError::Access(_))
    ));
    assert!(matches!(
        service
            .admit(&owner, None, None, &AgentId("missing".into()), meta())
            .await,
        Err(AttachmentError::Access(_))
    ));
    let admission = service
        .admit(&owner, None, None, &AgentId("agt_test".into()), meta())
        .await
        .unwrap();
    assert!(matches!(
        service.upload(admission, b"file".to_vec()).await,
        Err(AttachmentError::Unsupported) | Err(AttachmentError::NotReady)
    ));
}
#[tokio::test]
async fn attachment_admission_does_not_survive_owner_or_runtime_changes() {
    for change in [1, 2] {
        let (store, _, owner) = fixture().await;
        let changing = Arc::new(ChangingStore {
            store,
            change: std::sync::atomic::AtomicU8::new(0),
        });
        let service = AgentAttachmentService::new(
            Arc::new(ResourceAccessService::new(
                changing.clone(),
                Arc::new(NoOrganizations),
            )),
            Arc::new(UnusedRuntime {
                status: None,
                file_root: None,
            }),
        );
        let admission = service
            .admit(&owner, None, None, &AgentId("agt_test".into()), meta())
            .await
            .unwrap();
        changing
            .change
            .store(change, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            service.upload(admission, b"private".to_vec()).await,
            Err(AttachmentError::Access(_)) | Err(AttachmentError::Conflict)
        ));
    }
}
#[tokio::test]
async fn attachment_upload_checks_actual_byte_length_before_runtime_effect() {
    let (_, service, owner) = fixture().await;
    let admission = service
        .admit(&owner, None, None, &AgentId("agt_test".into()), meta())
        .await
        .unwrap();
    assert!(matches!(
        service
            .upload(admission, vec![0; MAX_ATTACHMENT_BYTES + 1])
            .await,
        Err(AttachmentError::TooLarge)
    ));
}

struct ReceiptWriter {
    mode: u8,
    changing: Arc<ChangingStore>,
}
#[async_trait::async_trait]
impl AttachmentWriter for ReceiptWriter {
    async fn write(
        &self,
        _: &Sandbox,
        attachment: &AgentAttachment,
        content: Vec<u8>,
    ) -> std::result::Result<AgentAttachment, AttachmentError> {
        assert_eq!(content, b"image");
        let mut receipt = attachment.clone();
        match self.mode {
            1 => receipt.agent_id = AgentId("foreign".into()),
            2 => receipt.path = "/tmp/other".into(),
            3 => receipt.size_bytes += 1,
            4 => self
                .changing
                .change
                .store(2, std::sync::atomic::Ordering::SeqCst),
            _ => {}
        }
        Ok(receipt)
    }
}
#[tokio::test]
async fn attachment_success_requires_matching_receipt_and_final_environment_fence() {
    for mode in 0..5 {
        let (store, _, owner) = fixture().await;
        let changing = Arc::new(ChangingStore {
            store,
            change: std::sync::atomic::AtomicU8::new(0),
        });
        let service = AgentAttachmentService::with_writer(
            Arc::new(ResourceAccessService::new(
                changing.clone(),
                Arc::new(NoOrganizations),
            )),
            Arc::new(ReceiptWriter { mode, changing }),
        );
        let admission = service
            .admit(&owner, None, None, &AgentId("agt_test".into()), meta())
            .await
            .unwrap();
        let result = service.upload(admission, b"image".to_vec()).await;
        if mode == 0 {
            let receipt = result.unwrap();
            assert!(receipt.path.starts_with("/home/attachments/agt_test/att_"));
            assert!(receipt.path.ends_with("/review.txt"));
            assert_eq!(receipt.size_bytes, 5);
        } else {
            assert!(matches!(result, Err(AttachmentError::Conflict)));
        }
    }
}

struct ChangingStore {
    store: Arc<store::InMemorySandboxStore>,
    change: std::sync::atomic::AtomicU8,
}
#[async_trait::async_trait]
impl ResourceAccessStore for ChangingStore {
    async fn organization_sandboxes(
        &self,
        organization: &SubjectId,
        query: &OwnerSandboxQuery,
    ) -> std::result::Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.store.organization_sandboxes(organization, query).await
    }
    async fn shared_sandboxes(
        &self,
        recipient: &SubjectId,
        organizations: &[SubjectId],
        query: &OwnerSandboxQuery,
    ) -> std::result::Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.store
            .shared_sandboxes(recipient, organizations, query)
            .await
    }
    async fn organization_agents(
        &self,
        organization: &SubjectId,
        query: &AgentListQuery,
        page: &store::PageRequest,
    ) -> std::result::Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.store
            .organization_agents(organization, query, page)
            .await
    }
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> std::result::Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        let mut snapshot = self.store.resource_access_snapshot(target).await?;
        match self.change.load(std::sync::atomic::Ordering::SeqCst) {
            1 => snapshot.lineage.sandbox.owner = SubjectId("foreign".into()),
            2 => {
                snapshot.lineage.sandbox.runtime = Some(RuntimeBinding {
                    boot_id: Some("f".repeat(32)),
                    runtime_id: "replacement".into(),
                    runtime_version: "image".into(),
                    provision_generation: Some(2),
                    bound_at: NOW.into(),
                })
            }
            _ => {}
        }
        Ok(snapshot)
    }
}
#[async_trait::async_trait]
impl ResourceShareStore for ChangingStore {
    async fn share_mutation_receipt(
        &self,
        id: &str,
    ) -> std::result::Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        self.store.share_mutation_receipt(id).await
    }
    async fn mutate_share(
        &self,
        request: ShareMutation,
    ) -> std::result::Result<ShareMutationReceipt, ResourceAccessStoreError> {
        self.store.mutate_share(request).await
    }
    async fn resource_share(
        &self,
        id: &str,
    ) -> std::result::Result<Option<ResourceShare>, ResourceAccessStoreError> {
        self.store.resource_share(id).await
    }
}

pub(super) fn persistent_home() -> SandboxHome {
    use layer3_adapter::agent_sandbox::*;
    let policy = HomeVolumePolicy {
        mode: HomeVolumeMode::NativeVctFullHomeV2,
        storage_class: "standard-rwo".into(),
        capacity_gib: 8,
        template_name: String::new(),
        template_generation: String::new(),
    };
    let mut home = SandboxHome::reserve(&SandboxId("sbx_test".into()), policy.clone()).unwrap();
    home.request.scope = Some(
        HomeVolumeScope::from_authority(
            "native-vct-empty-v1",
            &["attachment-test", "default", "runtime"],
        )
        .unwrap(),
    );
    let request = NativePoolClaimRequest {
        pool: NativePoolBinding {
            spec: NativePoolSpec {
                cold_canary: false,
                image: RuntimeImage {
                    uri: format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
                    labels: std::collections::BTreeMap::from([
                        (RUNTIME_CONTRACT_LABEL.into(), "3".into()),
                        ("dev.chrono.cma.runtime-bootstrap".into(), "1".into()),
                        ("dev.chrono.cma.home-layout".into(), "2".into()),
                    ]),
                },
                cpu_millis: 1000,
                memory_mib: 2048,
                home: policy,
            },
            pod_policy: NativePoolPodPolicy {
                runtime_class: None,
                service_account: "runtime".into(),
                image_pull_secrets: vec![],
                template_network_policy: None,
                fs_group_change_policy: None,
            },
            name: "pool".into(),
            uid: "pool-uid".into(),
            template_name: "template".into(),
            template_uid: "template-uid".into(),
            config_sha256: "a".repeat(64),
        },
        home: home.request.clone(),
        runtime_generation: 1,
    };
    let claim = NativePoolClaim {
        request: request.clone(),
        name: "claim".into(),
        uid: "claim-uid".into(),
        resource_version: "1".into(),
        sandbox_name: Some("member".into()),
    };
    home.native_pool = Some(Box::new(SandboxNativePool {
        request,
        claim: Some(claim.clone()),
        cleanup: None,
        cleaned: false,
    }));
    home.bind(HomeVolumeReceipt {
        native_authority: Some(Box::new(NativePoolHomeOrigin {
            claim,
            sandbox_uid: "sandbox-uid".into(),
            pvc_uid: "pvc-uid".into(),
        })),
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home-member".into(),
        storage_generation: "pvc-uid".into(),
    })
    .unwrap();
    home
}
#[tokio::test]
async fn attachment_runtime_writer_requires_actual_capability_and_exact_boot() {
    for mode in 0..3 {
        let store = Arc::new(store::InMemorySandboxStore::default());
        let home = persistent_home();
        let status = RuntimeStatus {
            agent_filesystem: false,
            agent_attachment_upload: mode != 1,
            agent_skill_install: false,
            agent_skill_upgrade: false,
            agent_model_catalog: false,
            agent_browser: false,
            agent_resource_preparation: false,
            agent_process_restart: false,
            exact_github_credential_route: false,
            github_repository_scope: false,
            sandbox_id: "sbx_test".into(),
            generation: 1,
            home_identity: home.attachment().unwrap().identity().unwrap(),
            boot_id: if mode == 2 { "c" } else { "b" }.repeat(32),
            installation_ready: true,
            credential_ready: true,
            restore_pending: false,
            agent_processes: 1,
        };
        let mut root = sandbox();
        root.runtime = Some(RuntimeBinding {
            boot_id: Some("b".repeat(32)),
            runtime_id: "runtime-test".into(),
            runtime_version: "image".into(),
            provision_generation: Some(1),
            bound_at: NOW.into(),
        });
        root.lifecycle.home = Some(Box::new(home));
        store.create_sandbox(root).await.unwrap();
        store.create_workspace(workspace()).await.unwrap();
        store.create_worktree(worktree()).await.unwrap();
        let mut a = agent();
        a.status = AgentStatus::Ready;
        a.conversation.conversation_id = Some("conversation".into());
        a.conversation.runtime_generation = Some(1);
        a.work.step = AgentWorkStep::None;
        store.create_agent(a).await.unwrap();
        let service = AgentAttachmentService::new(
            Arc::new(ResourceAccessService::new(store, Arc::new(NoOrganizations))),
            Arc::new(UnusedRuntime {
                status: Some(status),
                file_root: None,
            }),
        );
        let owner = Caller::new(
            SubjectId("owner".into()),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        let admission = service
            .admit(&owner, None, None, &AgentId("agt_test".into()), meta())
            .await
            .unwrap();
        let result = service.upload(admission, b"image".to_vec()).await;
        match mode {
            0 => assert!(result.is_ok(), "{result:?}"),
            1 => assert!(matches!(result, Err(AttachmentError::Unsupported))),
            _ => assert!(matches!(result, Err(AttachmentError::NotReady))),
        }
    }
}
