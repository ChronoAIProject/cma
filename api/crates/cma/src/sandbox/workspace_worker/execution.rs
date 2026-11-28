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
                tracing::debug!(
                    workspace_id = %id,
                    sandbox_id = %sandbox.id,
                    child_status = ?child.status,
                    child_step = ?child.work.step,
                    parent_wait_ms = started.elapsed().as_millis() as u64,
                    "Sandbox finished observing its Workspace bootstrap"
                );
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
        let started = tokio::time::Instant::now();
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
                    let at = if self.config.renewable_work
                        || current.work.authority_protocol.is_some()
                    {
                        now + started.elapsed()
                    } else {
                        now
                    };
                    if !self.run_one(id, at).await? {
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
            let result = self
                .dependencies
                .store
                .save_workspace(expected, workspace.clone())
                .await;
            tracing::debug!(
                workspace_id = %workspace.id,
                sandbox_id = %workspace.sandbox_id,
                step = ?workspace.work.step,
                claim_fence = workspace.work.lease.as_ref().map(|lease| lease.fence),
                store_result = super::authority::workspace_store_result(result.as_ref().err()),
                "Workspace execution policy publication ended"
            );
            result?;
        }
        Ok(workspace)
    }
}
