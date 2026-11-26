//! Wait for real parent readiness before resolving the reserved Worktree base.
use super::support::{
    claimed_fence, same_publication_claim, validate_lineage, PublicationParents, WorkerFailure,
};
use super::*;
use layer2_core::sandbox::store::{AgentWorkMutation, AgentWorkPublication, VersionedWorktree};
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
        let fallback = tokio::sync::Notify::new();
        let readiness = self.config.parent_readiness.as_deref().unwrap_or(&fallback);
        loop {
            // Register before reading so a commit during those reads cannot be
            // lost. This hint only wakes the same fresh lease/lineage checks.
            let changed = readiness.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let at = now + started.elapsed();
            let Some(current) = self.dependencies.store.get_agent(&agent.id).await? else {
                return Ok(());
            };
            if !same_publication_claim(&agent, &current, &self.config.holder, at) {
                return Ok(());
            }
            let (parent, workspace, worktree) = self.unstarted_lineage(&current).await?;
            if matches!(
                parent.status,
                SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired
            ) || matches!(
                workspace.status,
                WorkspaceStatus::Failed | WorkspaceStatus::Removing | WorkspaceStatus::Removed
            ) {
                return self
                    .handle_failure_with_parents(
                        current,
                        WorkerFailure::terminal(
                            "agent_parent_failed",
                            "The Sandbox or Workspace could not finish preparing.",
                        ),
                        at,
                        Some(PublicationParents {
                            sandbox: parent,
                            workspace,
                            worktree,
                        }),
                    )
                    .await;
            }
            let parents = PublicationParents {
                sandbox: parent,
                workspace,
                worktree,
            };
            if workspace_can_bind(&parents) {
                return self.bind_workspace(current, parents, at).await;
            }
            let remaining = budget.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                // The bounded observation already paced this expected wait. Keep
                // it due so committed parent readiness can wake it.
                return match self
                    .defer_workspace(current.clone(), parents.clone(), at, false)
                    .await
                {
                    Err(SandboxStoreError::Conflict)
                        if parents.workspace.status == WorkspaceStatus::Preparing =>
                    {
                        self.bind_newly_ready_workspace(current, parents, now + started.elapsed())
                            .await
                    }
                    result => result,
                };
            }
            tokio::select! {
                _ = &mut changed => {},
                // Other API replicas have no local notification; keep recovery bounded.
                _ = tokio::time::sleep(Duration::from_millis(250).min(remaining)) => {},
            }
        }
    }

    async fn bind_newly_ready_workspace(
        &self,
        original: Agent,
        observed: PublicationParents,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(current) = self.dependencies.store.get_agent(&original.id).await? else {
            return Ok(());
        };
        if !same_publication_claim(&original, &current, &self.config.holder, now) {
            return Ok(());
        }
        let (sandbox, workspace, worktree) = self.unstarted_lineage(&current).await?;
        let parents = PublicationParents {
            sandbox,
            workspace,
            worktree,
        };
        // Only cross the readiness boundary under the original runtime and
        // Workspace authority. The binding still fences all current parents.
        if !workspace_can_bind(&parents) || !observed.workspace_became_ready(&parents, &current) {
            return Err(SandboxStoreError::Conflict);
        }
        self.bind_workspace(current, parents, now).await
    }

    async fn defer_workspace(
        &self,
        agent: Agent,
        parents: PublicationParents,
        now: SystemTime,
        publication_aborted: bool,
    ) -> Result<(), SandboxStoreError> {
        self.publish_lifecycle_work(
            agent,
            Some(parents),
            now,
            |mut agent, at, parents| async move {
                let expected = agent.revision;
                let fence = claimed_fence(&agent, &self.config.holder)?;
                let stamp = rfc3339_at(at);
                let due = if publication_aborted {
                    // Repeated contention survives worker restarts without a hot loop.
                    at + Duration::from_secs(1 << agent.work.attempts.min(4))
                } else {
                    at
                };
                agent
                    .work
                    .retry(&self.config.holder, fence, rfc3339_at(due))
                    .map_err(|_| SandboxStoreError::Conflict)?;
                agent.updated_at = stamp.clone();
                agent.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
                self.commit_lifecycle_publication(
                    AgentWorkMutation {
                        expected_agent_revision: expected,
                        holder: self.config.holder.clone(),
                        fence,
                        agent,
                        worktree: None,
                        appended_at: stamp,
                        sandbox_event: None,
                        agent_logs: Vec::new(),
                    },
                    parents,
                )
                .await
            },
        )
        .await
    }

    async fn bind_workspace(
        &self,
        agent: Agent,
        parents: PublicationParents,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(commit) = parents.workspace.resolved_commit.clone() else {
            return Err(SandboxStoreError::Conflict);
        };
        let mut worktree = parents.worktree.clone();
        let expected_revision = worktree.revision;
        if !worktree.base_commit.is_empty() {
            return Err(SandboxStoreError::Conflict);
        }
        worktree.base_commit = commit;
        worktree.revision = expected_revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        let worktree = VersionedWorktree {
            expected_revision,
            worktree,
        };
        let started = tokio::time::Instant::now();
        let outcome = self
            .try_publish_lifecycle_work(
                agent.clone(),
                Some(parents.clone()),
                now,
                |agent, at, parents| {
                    let worktree = worktree.clone();
                    async move {
                        let mutation = self.progress_mutation(
                            agent,
                            AgentWorkStep::AddWorktree,
                            at,
                            None,
                            Vec::new(),
                            Some(worktree),
                            None,
                        )?;
                        self.commit_lifecycle_publication(mutation, parents).await
                    }
                },
            )
            .await?;
        match outcome {
            AgentWorkPublication::Published => Ok(()),
            AgentWorkPublication::KnownAborted => {
                self.defer_workspace(agent, parents, now + started.elapsed(), true)
                    .await
            }
        }
    }
}

fn workspace_can_bind(parents: &PublicationParents) -> bool {
    let sandbox = &parents.sandbox;
    sandbox.status == SandboxStatus::Active
        && sandbox.credential.delivered
        && sandbox.lifecycle.operation.is_none()
        && sandbox.lifecycle.checkpoint_capture.is_none()
        && parents.workspace.status == WorkspaceStatus::Ready
}
