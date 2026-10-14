//! Wait for real parent readiness before resolving the reserved Worktree base.
use super::support::{claimed_fence, validate_lineage, WorkerFailure};
use super::*;
use layer2_core::sandbox::store::{AgentWorkMutation, VersionedWorktree};
use layer2_core::sandbox::{Sandbox, SandboxStatus, Workspace, WorkspaceStatus, Worktree};

impl AgentWorker {
    pub(super) async fn unstarted_lineage(
        &self,
        agent: &Agent,
    ) -> Result<(Sandbox, Workspace, Worktree), SandboxStoreError> {
        if !agent.has_no_runtime_effects() {
            return Err(SandboxStoreError::Conflict);
        }
        let (parent, workspace, worktree) = tokio::join!(
            self.dependencies.store.get_sandbox(&agent.sandbox_id),
            self.dependencies.store.get_workspace(&agent.workspace_id),
            self.dependencies.store.get_worktree(&agent.worktree_id)
        );
        let parent = parent?.ok_or(SandboxStoreError::NotFound)?;
        let workspace = workspace?.ok_or(SandboxStoreError::NotFound)?;
        let worktree = worktree?.ok_or(SandboxStoreError::NotFound)?;
        validate_lineage(agent, &parent, &workspace, &worktree)
            .map_err(|e| SandboxStoreError::Corrupt(e.to_string()))?;
        Ok((parent, workspace, worktree))
    }

    pub(super) async fn wait_for_workspace(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        // Parent preparation is expected work. Observe it under this claim for
        // a bounded interval without publishing failure logs or runtime effects.
        let started = tokio::time::Instant::now();
        let budget = Duration::from_secs(10).min(self.config.lease_duration / 3);
        loop {
            let current = self.dependencies.store.get_agent(&agent.id).await?;
            if current.as_ref().is_none_or(|current| {
                current.revision != agent.revision
                    || current.work.lease != agent.work.lease
                    || current.work.step != AgentWorkStep::WaitForWorkspace
                    || current.requested_stop_reason.is_some()
            }) {
                // A Stop or a newer claim owns the next action.
                return Ok(());
            }
            let at = now + started.elapsed();
            let (parent, workspace, _) = self.unstarted_lineage(&agent).await?;
            if matches!(
                parent.status,
                SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired
            ) || matches!(
                workspace.status,
                WorkspaceStatus::Failed | WorkspaceStatus::Removing | WorkspaceStatus::Removed
            ) {
                return self
                    .handle_failure(
                        agent,
                        WorkerFailure::terminal(
                            "agent_parent_failed",
                            "The Sandbox or Workspace could not finish preparing.",
                        ),
                        at,
                    )
                    .await;
            }
            if parent.status == SandboxStatus::Active
                && parent.credential.delivered
                && parent.lifecycle.operation.is_none()
                && parent.lifecycle.checkpoint_capture.is_none()
                && workspace.status == WorkspaceStatus::Ready
            {
                return self.bind_workspace(agent, at).await;
            }
            let remaining = budget.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return self.defer_workspace(agent, at).await;
            }
            tokio::time::sleep(Duration::from_millis(250).min(remaining)).await;
        }
    }

    async fn defer_workspace(
        &self,
        mut agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let stamp = rfc3339_at(now);
        // The bounded observation already paced this expected wait. Leave it
        // due so committed parent readiness can wake it without another delay.
        // Discovery and claims still check durable due time, lease and lineage.
        agent
            .work
            .retry(&self.config.holder, fence, stamp.clone())
            .map_err(|_| SandboxStoreError::Conflict)?;
        agent.updated_at = stamp.clone();
        agent.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
        self.dependencies
            .store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: expected,
                holder: self.config.holder.clone(),
                fence,
                agent,
                worktree: None,
                appended_at: stamp,
                sandbox_event: None,
                agent_logs: Vec::new(),
            })
            .await
    }

    async fn bind_workspace(&self, agent: Agent, now: SystemTime) -> Result<(), SandboxStoreError> {
        let (parent, workspace, mut worktree) = self.unstarted_lineage(&agent).await?;
        if matches!(
            parent.status,
            SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired
        ) || matches!(
            workspace.status,
            WorkspaceStatus::Failed | WorkspaceStatus::Removing | WorkspaceStatus::Removed
        ) {
            return self
                .handle_failure(
                    agent,
                    WorkerFailure::terminal(
                        "agent_parent_failed",
                        "The Sandbox or Workspace could not finish preparing.",
                    ),
                    now,
                )
                .await;
        }
        if parent.status != SandboxStatus::Active
            || !parent.credential.delivered
            || parent.lifecycle.operation.is_some()
            || parent.lifecycle.checkpoint_capture.is_some()
            || workspace.status != WorkspaceStatus::Ready
        {
            return self.defer_workspace(agent, now).await;
        }
        let Some(commit) = workspace.resolved_commit else {
            return Err(SandboxStoreError::Conflict);
        };
        let expected_revision = worktree.revision;
        if !worktree.base_commit.is_empty() {
            return Err(SandboxStoreError::Conflict);
        }
        worktree.base_commit = commit;
        worktree.revision = expected_revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        // The transaction repeats the parent readiness and exact commit checks,
        // and write-fences both parent records before publishing AddWorktree.
        self.progress(
            agent,
            AgentWorkStep::AddWorktree,
            now,
            None,
            Vec::new(),
            Some(VersionedWorktree {
                expected_revision,
                worktree,
            }),
            None,
        )
        .await
    }
}
