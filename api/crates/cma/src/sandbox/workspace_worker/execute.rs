use super::*;
impl WorkspaceWorker {
    async fn execute(
        &self,
        workspace: Workspace,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if workspace.work.step == WorkspaceWorkStep::Remove {
            let parent = self
                .dependencies
                .store
                .get_sandbox(&workspace.sandbox_id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?;
            if parent.retirement_archive_pending() {
                return Err(SandboxStoreError::Conflict);
            }
        }
        match workspace.work.step {
            WorkspaceWorkStep::Prepare => self.prepare(workspace, now).await,
            WorkspaceWorkStep::RecoverPrepare => self.recover_prepare(workspace, now).await,
            WorkspaceWorkStep::Setup => self.setup(workspace, now).await,
            WorkspaceWorkStep::Remove => self.remove(workspace, now).await,
            WorkspaceWorkStep::None => Err(SandboxStoreError::Conflict),
        }
    }
}
