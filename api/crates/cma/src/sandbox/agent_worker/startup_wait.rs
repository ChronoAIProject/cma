//! Wait for real parent readiness before resolving the reserved Worktree base.
use super::support::{
    claimed_fence, same_publication_claim, validate_lineage, PublicationParents, WorkerFailure,
};
use super::*;
use layer2_core::sandbox::store::{AgentWorkMutation, AgentWorkPublication, VersionedWorktree};
use layer2_core::sandbox::{
    AgentStatus, Sandbox, SandboxStatus, Workspace, WorkspaceOrigin, WorkspaceStatus, Worktree,
};

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
            let Some(current) = self.dependencies.store.get_agent(&agent.id).await? else {
                return Ok(());
            };
            if !same_publication_claim(
                &agent,
                &current,
                &self.config.holder,
                now + started.elapsed(),
            ) {
                return Ok(());
            }
            let (parent, workspace, worktree) = self.unstarted_lineage(&current).await?;
            let at = now + started.elapsed();
            if !same_publication_claim(&agent, &current, &self.config.holder, at) {
                return Ok(());
            }
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
            let ready = workspace_can_bind(&parents);
            let remaining = budget.saturating_sub(started.elapsed());
            if ready || remaining.is_zero() {
                let result = if ready {
                    box_wait_publication(|| {
                        self.bind_workspace(current.clone(), parents.clone(), at)
                    })
                    .await
                } else {
                    // The bounded observation already paced this expected wait.
                    box_wait_publication(|| {
                        self.defer_workspace(current.clone(), parents.clone(), at, false)
                    })
                    .await
                };
                let known_abort = match result {
                    Ok(AgentWorkPublication::Published) => return Ok(()),
                    Ok(AgentWorkPublication::KnownAborted) => true,
                    Err(SandboxStoreError::Conflict) => false,
                    Err(error) => return Err(error),
                };
                return box_wait_publication(|| {
                    self.recover_workspace_wait(
                        current,
                        parents,
                        now + started.elapsed(),
                        known_abort,
                    )
                })
                .await;
            }
            tokio::select! {
                _ = &mut changed => {},
                // Other API replicas have no local notification; keep recovery bounded.
                _ = tokio::time::sleep(Duration::from_millis(250).min(remaining)) => {},
            }
        }
    }

    async fn recover_workspace_wait(
        &self,
        original: Agent,
        observed: PublicationParents,
        now: SystemTime,
        mut known_abort: bool,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let mut requeue_parent = observed.clone();
        let mut tried_binding = workspace_can_bind(&observed);
        let mut recovering = known_abort;
        // Credential delivery, Workspace readiness and Sandbox activation can
        // publish on different replicas while the original wait is requeued.
        for _ in 0..3 {
            let Some(current) = self.dependencies.store.get_agent(&original.id).await? else {
                return Ok(());
            };
            if !same_publication_claim(
                &original,
                &current,
                &self.config.holder,
                now + started.elapsed(),
            ) {
                return Ok(());
            }
            let (sandbox, workspace, worktree) = self.unstarted_lineage(&current).await?;
            let at = now + started.elapsed();
            if !same_publication_claim(&original, &current, &self.config.holder, at) {
                return Ok(());
            }
            let parents = PublicationParents {
                sandbox,
                workspace,
                worktree,
            };
            // A bind can reuse this claim only under its original authority.
            if !tried_binding
                && workspace_can_bind(&parents)
                && observed.workspace_became_ready(&parents, &current)
            {
                tried_binding = true;
                match self.bind_workspace(current, parents.clone(), at).await {
                    Ok(AgentWorkPublication::Published) => return Ok(()),
                    Ok(AgentWorkPublication::KnownAborted) => known_abort = true,
                    Err(SandboxStoreError::Conflict) => {}
                    Err(error) => return Err(error),
                }
                recovering = true;
                requeue_parent = parents;
                continue;
            }
            let parent_advanced = parents.sandbox.revision > requeue_parent.sandbox.revision
                || parents.workspace.revision > requeue_parent.workspace.revision;
            if !same_bootstrap_lineage(&requeue_parent, &parents, &current)
                || !(recovering || parent_advanced || current.revision > original.revision)
            {
                return Err(SandboxStoreError::Conflict);
            }
            let fence = claimed_fence(&current, &self.config.holder)?;
            // Contention can abort this transaction without changing a parent
            // revision. A fresh compatible snapshot can still release the claim.
            recovering = true;
            match self
                .defer_workspace(current, parents.clone(), at, known_abort)
                .await
            {
                Ok(AgentWorkPublication::Published) => {
                    tracing::info!(
                        agent_id = %original.id,
                        authority_fence = fence,
                        known_abort,
                        "Agent workspace wait claim released after publication conflict"
                    );
                    return Ok(());
                }
                Ok(AgentWorkPublication::KnownAborted) => known_abort = true,
                Err(SandboxStoreError::Conflict) => {}
                Err(error) => return Err(error),
            }
            requeue_parent = parents;
        }
        Err(SandboxStoreError::Conflict)
    }

    async fn defer_workspace(
        &self,
        agent: Agent,
        parents: PublicationParents,
        now: SystemTime,
        publication_aborted: bool,
    ) -> Result<AgentWorkPublication, SandboxStoreError> {
        self.try_publish_lifecycle_work(
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
    ) -> Result<AgentWorkPublication, SandboxStoreError> {
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
        self.try_publish_lifecycle_work(agent, Some(parents), now, |agent, at, parents| {
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
        })
        .await
    }
}

// Keep each publication constructor off the observation frame. The store
// still polls the selected publication under the original work authority.
#[inline(never)]
fn box_wait_publication<F: Future>(start: impl FnOnce() -> F) -> Pin<Box<F>> {
    Box::pin(start())
}

fn workspace_can_bind(parents: &PublicationParents) -> bool {
    let sandbox = &parents.sandbox;
    sandbox.status == SandboxStatus::Active
        && sandbox.credential.delivered
        && sandbox.lifecycle.operation.is_none()
        && sandbox.lifecycle.checkpoint_capture.is_none()
        && parents.workspace.status == WorkspaceStatus::Ready
}

// Only the original cold-start lineage can become due again on these fresh
// observations. The next claim repeats all authority checks before binding.
fn same_bootstrap_lineage(
    observed: &PublicationParents,
    current: &PublicationParents,
    agent: &Agent,
) -> bool {
    let before = &observed.sandbox;
    let after = &current.sandbox;
    let workspace = &current.workspace;
    matches!(
        (before.status, after.status),
        (
            SandboxStatus::Provisioning,
            SandboxStatus::Provisioning | SandboxStatus::Installing | SandboxStatus::Active
        ) | (
            SandboxStatus::Installing,
            SandboxStatus::Installing | SandboxStatus::Active
        ) | (SandboxStatus::Active, SandboxStatus::Active)
    ) && agent.status == AgentStatus::Starting
        && agent.work.step == AgentWorkStep::WaitForWorkspace
        && after.lifecycle.operation.is_none()
        && after.lifecycle.checkpoint_capture.is_none()
        && after.lifecycle.suspended_runtime.is_none()
        && after.retired_at.is_none()
        && after.failure.is_none()
        && (after.status != SandboxStatus::Active || after.credential.delivered)
        && after.origin == before.origin
        && after.profile == before.profile
        && !after.retirement_archive_pending()
        && before
            .runtime
            .as_ref()
            .is_none_or(|runtime| after.runtime.as_ref() == Some(runtime))
        && (!before.credential.delivered || after.credential.delivered)
        && matches!(workspace.origin, WorkspaceOrigin::SandboxProfile { .. })
        && workspace.origin_matches_sandbox_profile(after)
        && workspace.origin == observed.workspace.origin
        && workspace.profile == observed.workspace.profile
        && workspace.source == observed.workspace.source
        && observed
            .workspace
            .resolved_commit
            .as_ref()
            .is_none_or(|commit| workspace.resolved_commit.as_ref() == Some(commit))
        && (observed.workspace.status != WorkspaceStatus::Ready
            || workspace.status == WorkspaceStatus::Ready)
        && matches!(
            workspace.status,
            WorkspaceStatus::Preparing | WorkspaceStatus::Ready
        )
        && workspace.failed.is_none()
        && workspace.removed_at.is_none()
        && current.worktree == observed.worktree
        && current.worktree.base_commit.is_empty()
        && after.revision >= before.revision
        && workspace.revision >= observed.workspace.revision
}
