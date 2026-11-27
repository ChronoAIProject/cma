use super::*;

#[async_trait]
impl WorkspaceStore for InMemorySandboxStore {
    async fn create_workspace(&self, workspace: Workspace) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        if state.workspaces.values().any(|stored| {
            stored.sandbox_id == workspace.sandbox_id
                && stored.name == workspace.name
                && stored.removed_at.is_none()
        }) {
            return Err(SandboxStoreError::Duplicate);
        }
        let id = workspace.id.clone();
        create_record!(state, workspaces, id, workspace)
    }

    async fn get_workspace(
        &self,
        id: &WorkspaceId,
    ) -> Result<Option<Workspace>, SandboxStoreError> {
        Ok(self.lock().workspaces.get(id).cloned())
    }

    async fn save_workspace(
        &self,
        expected_revision: u64,
        workspace: Workspace,
    ) -> Result<(), SandboxStoreError> {
        let id = workspace.id.clone();
        let mut state = self.lock();
        if state.workspaces.get(&id).is_some_and(|current| {
            crate::sandbox::ProvisioningExecution::validate_successor(
                current.work.execution,
                workspace.work.execution,
            )
            .is_err()
        }) {
            return Err(SandboxStoreError::Conflict);
        }
        save_record!(state, workspaces, id, expected_revision, workspace)
    }

    async fn list_workspaces(
        &self,
        sandbox_id: &SandboxId,
        request: &PageRequest,
    ) -> Result<Page<Workspace>, SandboxStoreError> {
        let values = self
            .lock()
            .workspaces
            .values()
            .filter(|workspace| &workspace.sandbox_id == sandbox_id)
            .map(|workspace| (workspace.id.to_string(), workspace.clone()))
            .collect();
        page(values, request)
    }
}
