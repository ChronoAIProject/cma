use super::*;

impl WorkspaceWorker {
    pub async fn run_until_ready(
        &self,
        id: &WorkspaceId,
        now: SystemTime,
    ) -> Result<WorkspaceDrain, SandboxStoreError> {
        let mut executed_work = false;
        for _ in 0..4 {
            let current = self
                .dependencies
                .store
                .get_workspace(id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?;
            match current.status {
                WorkspaceStatus::Ready
                | WorkspaceStatus::Failed
                | WorkspaceStatus::Removing
                | WorkspaceStatus::Removed => {
                    return Ok(WorkspaceDrain {
                        workspace: current,
                        executed_work,
                    });
                }
                WorkspaceStatus::Preparing => {
                    if !self.run_one(id, now).await? {
                        return Ok(WorkspaceDrain {
                            workspace: current,
                            executed_work,
                        });
                    }
                    executed_work = true;
                }
            }
        }
        let workspace = self
            .dependencies
            .store
            .get_workspace(id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        Ok(WorkspaceDrain {
            workspace,
            executed_work,
        })
    }

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
