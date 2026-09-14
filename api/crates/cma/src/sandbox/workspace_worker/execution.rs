use super::*;

impl WorkspaceWorker {
    pub(super) async fn pin_execution(
        &self,
        mut workspace: Workspace,
    ) -> Result<Workspace, SandboxStoreError> {
        if workspace.work.execution.is_none() {
            claimed_fence(&workspace, &self.config.holder)?;
            let expected = workspace.revision;
            workspace.work.execution = Some(self.config.execution);
            workspace.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            self.dependencies
                .store
                .save_workspace(expected, workspace.clone())
                .await?;
        }
        Ok(workspace)
    }
}
