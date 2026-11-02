use super::*;

pub(super) struct BufferedStore {
    pub(super) inner: Arc<InMemorySandboxStore>,
    pub(super) sandbox: Option<SandboxHierarchyRecord>,
    pub(super) dependency_failure: bool,
    pub(super) gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}
#[async_trait::async_trait]
impl HierarchyReadStore for BufferedStore {
    async fn read_snapshot_archive(
        &self,
        _: &SandboxId,
        _: &SandboxCheckpointRef,
    ) -> Result<SandboxSnapshotDownload, SandboxStoreError> {
        unreachable!()
    }
    async fn list_owner_sandbox_inventory(
        &self,
        owner: &SubjectId,
        status: Option<SandboxStatus>,
        exclude_retired: bool,
        page: &PageRequest,
    ) -> Result<Page<SandboxInventoryRecord>, SandboxStoreError> {
        self.inner
            .list_owner_sandbox_inventory(owner, status, exclude_retired, page)
            .await
    }
    async fn read_sandbox_hierarchy(
        &self,
        id: &SandboxId,
    ) -> Result<Option<SandboxHierarchyRecord>, SandboxStoreError> {
        let result = match &self.sandbox {
            Some(value) => Ok(Some(value.clone())),
            None => self.inner.read_sandbox_hierarchy(id).await,
        };
        if let Some((entered, resume)) = &self.gate {
            entered.notify_one();
            resume.notified().await;
        }
        if self.dependency_failure {
            Err(SandboxStoreError::Unavailable("private-dependency".into()))
        } else {
            result
        }
    }
    async fn read_workspace_hierarchy(
        &self,
        id: &WorkspaceId,
    ) -> Result<Option<WorkspaceHierarchyRecord>, SandboxStoreError> {
        let result = self.inner.read_workspace_hierarchy(id).await;
        if let Some((entered, resume)) = &self.gate {
            entered.notify_one();
            resume.notified().await;
        }
        if self.dependency_failure {
            Err(SandboxStoreError::Unavailable("private-dependency".into()))
        } else {
            result
        }
    }
    async fn read_hierarchy_worktree(
        &self,
        id: &WorktreeId,
    ) -> Result<Option<Worktree>, SandboxStoreError> {
        self.inner.read_hierarchy_worktree(id).await
    }
}
