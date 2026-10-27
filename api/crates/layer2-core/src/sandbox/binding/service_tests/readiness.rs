use super::*;
use crate::sandbox::store::{Page, PageRequest, SandboxStoreError};

async fn orphan(f: &Fixture, with_sandbox: bool) -> ProviderBinding {
    let mut binding = f.put("binding-one", "Original instruction").await;
    if with_sandbox {
        f.service.reconcile_binding(&binding).await.unwrap();
    }
    let revision = binding.revision;
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding.advance_revision(crate::now_rfc3339()).unwrap();
    f.store
        .save_binding(Some(revision), binding.clone())
        .await
        .unwrap();
    binding
}

#[tokio::test]
async fn stale_ready_binding_refuses_read_replay_rotation_and_conversation_without_mutation() {
    for with_sandbox in [false, true] {
        let f = fixture();
        let binding = orphan(&f, with_sandbox).await;
        let sandbox = f.store.get_sandbox(&binding.sandbox_id).await.unwrap();
        let keys = f.custody.keys.lock().unwrap().clone();
        assert!(matches!(
            f.service.read(&f.caller, &binding.binding_id).await,
            Err(BindingError::ResourcesUnavailable)
        ));
        assert!(matches!(
            f.service
                .ensure_conversation(&f.caller, &binding.binding_id, "conversation_test")
                .await,
            Err(BindingError::ResourcesUnavailable)
        ));
        let input = profile("Original instruction");
        let digest = format!("sha256:{}", content_digest(&input).unwrap());
        let replacement =
            ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
        for key in [&f.key, &replacement] {
            assert!(matches!(
                f.service
                    .put(&f.caller, &binding.binding_id, input.clone(), &digest, key)
                    .await,
                Err(BindingError::ResourcesUnavailable)
            ));
        }
        assert_eq!(
            f.store.binding(&binding.binding_id).await.unwrap(),
            Some(binding.clone())
        );
        assert_eq!(
            f.store.get_sandbox(&binding.sandbox_id).await.unwrap(),
            sandbox
        );
        assert!(f
            .store
            .get_workspace(&binding.workspace_id)
            .await
            .unwrap()
            .is_none());
        assert!(f
            .store
            .binding_conversations(&binding.binding_id, binding.generation, None, 100)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(*f.custody.keys.lock().unwrap(), keys);

        let other = Caller::new(SubjectId::new("other"), f.caller.authorization.clone());
        assert!(matches!(
            f.service.read(&other, &binding.binding_id).await,
            Err(BindingError::NotFound)
        ));
        f.service
            .retire_conversation(&f.caller, &binding.binding_id, "conversation_absent")
            .await
            .unwrap();
        assert_eq!(
            f.store.binding(&binding.binding_id).await.unwrap(),
            Some(binding.clone())
        );
        f.service
            .retire(&f.caller, &binding.binding_id)
            .await
            .unwrap();
        assert_eq!(
            f.store
                .binding(&binding.binding_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            BindingState::Retired
        );
    }
}

#[tokio::test]
async fn removing_resources_refuse_new_work_without_recreating_or_changing_history() {
    for sandbox_status in [
        Some(SandboxStatus::Retiring),
        Some(SandboxStatus::Retired),
        None,
    ] {
        for workspace_status in [WorkspaceStatus::Removing, WorkspaceStatus::Removed] {
            let f = fixture();
            let binding = f.ready().await;
            let (conversation, _) = f
                .service
                .ensure_conversation(&f.caller, &binding.binding_id, "conversation_existing")
                .await
                .unwrap();
            let mut sandbox = f
                .store
                .get_sandbox(&binding.sandbox_id)
                .await
                .unwrap()
                .unwrap();
            if let Some(status) = sandbox_status {
                let revision = sandbox.revision;
                sandbox.status = status;
                sandbox.revision += 1;
                f.store
                    .save_sandbox(revision, sandbox.clone())
                    .await
                    .unwrap();
            } else {
                let mut workspace = f
                    .store
                    .get_workspace(&binding.workspace_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = workspace.revision;
                workspace.status = workspace_status;
                workspace.revision += 1;
                f.store.save_workspace(revision, workspace).await.unwrap();
            }
            let workspace = f.store.get_workspace(&binding.workspace_id).await.unwrap();
            assert!(matches!(
                f.service.read(&f.caller, &binding.binding_id).await,
                Err(BindingError::ResourcesUnavailable)
            ));
            assert!(matches!(
                f.service
                    .ensure_conversation(
                        &f.caller,
                        &binding.binding_id,
                        &conversation.conversation_id
                    )
                    .await,
                Err(BindingError::ResourcesUnavailable)
            ));
            assert_eq!(
                f.store.binding(&binding.binding_id).await.unwrap(),
                Some(binding.clone())
            );
            assert_eq!(
                f.store.get_sandbox(&binding.sandbox_id).await.unwrap(),
                Some(sandbox)
            );
            assert_eq!(
                f.store.get_workspace(&binding.workspace_id).await.unwrap(),
                workspace
            );
            assert_eq!(
                f.store
                    .binding_conversation(
                        &binding.binding_id,
                        binding.generation,
                        &conversation.conversation_id
                    )
                    .await
                    .unwrap(),
                Some(conversation)
            );
        }
    }
}

#[tokio::test]
async fn paused_and_recovering_binding_resources_keep_their_identity_and_state() {
    let f = fixture();
    let binding = f.ready().await;
    let (conversation, _) = f
        .service
        .ensure_conversation(&f.caller, &binding.binding_id, "conversation_existing")
        .await
        .unwrap();
    for status in [
        SandboxStatus::Active,
        SandboxStatus::Pausing,
        SandboxStatus::Paused,
        SandboxStatus::Reviving,
        SandboxStatus::CredentialRotating,
        SandboxStatus::Rotating,
        SandboxStatus::Failed,
    ] {
        let mut sandbox = f
            .store
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let revision = sandbox.revision;
        sandbox.status = status;
        sandbox.revision += 1;
        f.store
            .save_sandbox(revision, sandbox.clone())
            .await
            .unwrap();
        assert_eq!(
            f.service
                .read(&f.caller, &binding.binding_id)
                .await
                .unwrap(),
            binding
        );
        assert_eq!(
            f.service
                .ensure_conversation(
                    &f.caller,
                    &binding.binding_id,
                    &conversation.conversation_id
                )
                .await
                .unwrap(),
            (conversation.clone(), false)
        );
        assert_eq!(
            f.store.get_sandbox(&binding.sandbox_id).await.unwrap(),
            Some(sandbox)
        );
    }
}

#[tokio::test]
async fn backing_resource_owner_mismatch_is_not_absence_or_usable_readiness() {
    let f = fixture();
    let binding = f.ready().await;
    let mut mismatched = binding.clone();
    mismatched.owner = SubjectId::new("another-owner");
    assert!(matches!(
        super::super::readiness::validate(f.store.as_ref(), &mismatched).await,
        Err(BindingError::Corrupt(_))
    ));
    assert_eq!(
        f.store.binding(&binding.binding_id).await.unwrap(),
        Some(binding)
    );
}

struct FailingReads {
    sandbox_fails: bool,
}
#[async_trait::async_trait]
impl SandboxStore for FailingReads {
    async fn get_sandbox(&self, _: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        if self.sandbox_fails {
            Err(SandboxStoreError::Unavailable("test outage".into()))
        } else {
            Ok(None)
        }
    }
    async fn create_sandbox(&self, _: Sandbox) -> Result<(), SandboxStoreError> {
        panic!("readiness must not write")
    }
    async fn save_sandbox(&self, _: u64, _: Sandbox) -> Result<(), SandboxStoreError> {
        panic!("readiness must not write")
    }
    async fn list_sandboxes(
        &self,
        _: &SubjectId,
        _: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        panic!("readiness must not scan")
    }
}
#[async_trait::async_trait]
impl WorkspaceStore for FailingReads {
    async fn get_workspace(
        &self,
        _: &crate::WorkspaceId,
    ) -> Result<Option<Workspace>, SandboxStoreError> {
        if self.sandbox_fails {
            Ok(None)
        } else {
            Err(SandboxStoreError::Unavailable("test outage".into()))
        }
    }
    async fn create_workspace(&self, _: Workspace) -> Result<(), SandboxStoreError> {
        panic!("readiness must not write")
    }
    async fn save_workspace(&self, _: u64, _: Workspace) -> Result<(), SandboxStoreError> {
        panic!("readiness must not write")
    }
    async fn list_workspaces(
        &self,
        _: &SandboxId,
        _: &PageRequest,
    ) -> Result<Page<Workspace>, SandboxStoreError> {
        panic!("readiness must not scan")
    }
}

#[tokio::test]
async fn storage_failure_is_not_misreported_as_a_deleted_binding_environment() {
    let f = fixture();
    let binding = orphan(&f, false).await;
    for sandbox_fails in [true, false] {
        assert!(matches!(
            super::super::readiness::validate(&FailingReads { sandbox_fails }, &binding).await,
            Err(BindingError::Unavailable(_))
        ));
    }
}
