//! Wait for real parent readiness before resolving the reserved Worktree base.
use super::support::{validate_lineage, WorkerFailure};
use super::*;
use layer2_core::sandbox::store::VersionedWorktree;
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
            return self
                .handle_failure(
                    agent,
                    WorkerFailure::retryable(
                        "agent_parent_not_ready",
                        "The Sandbox and Workspace are still preparing.",
                    ),
                    now,
                )
                .await;
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
