use super::*;

impl WorkspaceWorker {
    pub(in crate::sandbox) async fn run_until_ready_observing_bootstrap(
        &self,
        id: &WorkspaceId,
        sandbox: &Sandbox,
        now: SystemTime,
    ) -> Result<WorkspaceDrain, SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let mut executed_work = false;
        loop {
            let at = now + started.elapsed();
            let mut drained = self.run_until_ready(id, at).await?;
            executed_work |= drained.executed_work;
            drained.executed_work = executed_work;
            let child = &drained.workspace;
            let observing_bootstrap = matches!(
                sandbox.origin,
                layer2_core::sandbox::SandboxOrigin::Lifecycle { .. }
            ) && child.id == *id
                && child.owner == sandbox.owner
                && child.sandbox_id == sandbox.id
                && matches!(
                    child.origin,
                    layer2_core::sandbox::WorkspaceOrigin::SandboxProfile { .. }
                )
                && child.origin_matches_sandbox_profile(sandbox)
                && child.status == WorkspaceStatus::Preparing
                && child.work.attempts == 0
                && child.work.ready_at <= rfc3339_at(at);
            let remaining = Duration::from_secs(10).saturating_sub(started.elapsed());
            if !observing_bootstrap || remaining.is_zero() {
                return Ok(drained);
            }
            // An independent scheduler may own this exact child's Prepare or
            // Setup claim. Observe its durable publication without a competing
            // effect; the normal claim transaction still decides each attempt.
            tokio::time::sleep(Duration::from_millis(250).min(remaining)).await;
        }
    }

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
