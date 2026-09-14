use super::{
    super::{hierarchy_router, instances_tests, HierarchyState},
    Scenario,
};
use layer2_core::sandbox::store::{
    InMemorySandboxStore, Page, PageRequest, SandboxStore, SandboxStoreError,
};
use layer2_core::sandbox::{
    HierarchyReadService, HierarchyReadStore, SandboxCheckpointRef, SandboxHierarchyRecord,
    SandboxInventoryRecord, SandboxSnapshotDownload, SandboxStatus, WorkspaceHierarchyRecord,
    Worktree,
};
use layer2_core::{Authorization, Capability, SandboxId, SubjectId, WorkspaceId, WorktreeId};
use std::sync::Arc;

const ARCHIVE: &[u8] = b"snapshot transport fixture";

struct SnapshotStore {
    inner: Arc<InMemorySandboxStore>,
    unavailable: bool,
}

#[async_trait::async_trait]
impl HierarchyReadStore for SnapshotStore {
    async fn read_snapshot_archive(
        &self,
        sandbox_id: &SandboxId,
        checkpoint: &SandboxCheckpointRef,
    ) -> Result<SandboxSnapshotDownload, SandboxStoreError> {
        assert_eq!(sandbox_id.as_str(), "sbx_snapshot");
        assert_eq!(checkpoint.epoch, 1);
        if self.unavailable {
            return Err(SandboxStoreError::Unavailable(
                "snapshot store unavailable".into(),
            ));
        }
        let (send, chunks) = tokio::sync::mpsc::channel(2);
        send.send(Ok(ARCHIVE[..8].to_vec())).await.unwrap();
        send.send(Ok(ARCHIVE[8..].to_vec())).await.unwrap();
        Ok(SandboxSnapshotDownload {
            size_bytes: ARCHIVE.len() as u64,
            chunks,
        })
    }
    async fn list_owner_sandbox_inventory(
        &self,
        owner: &SubjectId,
        status: Option<SandboxStatus>,
        page: &PageRequest,
    ) -> Result<Page<SandboxInventoryRecord>, SandboxStoreError> {
        self.inner
            .list_owner_sandbox_inventory(owner, status, page)
            .await
    }
    async fn read_sandbox_hierarchy(
        &self,
        id: &SandboxId,
    ) -> Result<Option<SandboxHierarchyRecord>, SandboxStoreError> {
        self.inner.read_sandbox_hierarchy(id).await
    }
    async fn read_workspace_hierarchy(
        &self,
        id: &WorkspaceId,
    ) -> Result<Option<WorkspaceHierarchyRecord>, SandboxStoreError> {
        self.inner.read_workspace_hierarchy(id).await
    }
    async fn read_hierarchy_worktree(
        &self,
        id: &WorktreeId,
    ) -> Result<Option<Worktree>, SandboxStoreError> {
        self.inner.read_hierarchy_worktree(id).await
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if ![
        "snapshot_owner",
        "snapshot_forbidden",
        "snapshot_anonymous",
        "snapshot_foreign",
        "snapshot_unavailable",
    ]
    .contains(&name)
    {
        return None;
    }
    let store = Arc::new(InMemorySandboxStore::default());
    let mut sandbox = instances_tests::paused_sandbox("sbx_snapshot", "owner");
    sandbox.lifecycle.checkpoints.latest = Some(instances_tests::checkpoint(1));
    store.create_sandbox(sandbox).await.unwrap();
    let profiles = Arc::new(layer2_core::profiles::store::InMemoryProfileStore::default());
    let sandboxes = layer2_core::sandbox::SandboxService::new(
        store.clone(),
        profiles.clone(),
        Default::default(),
        Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let workspaces = layer2_core::sandbox::WorkspaceService::new(store.clone(), profiles);
    let hierarchy = HierarchyReadService::new(Arc::new(SnapshotStore {
        inner: store,
        unavailable: name == "snapshot_unavailable",
    }));
    Some(Scenario {
        router: hierarchy_router(HierarchyState::new(
            sandboxes,
            workspaces,
            hierarchy,
            Arc::new(instances_tests::MemoryOperationStore::default()),
        )),
        subject: match name {
            "snapshot_anonymous" => None,
            "snapshot_foreign" => Some("stranger"),
            _ => Some("owner"),
        },
        authorization: Authorization::new(if name == "snapshot_forbidden" {
            vec![]
        } else {
            vec![Capability::UseOwnSandboxes]
        }),
        symbols: Default::default(),
    })
}
