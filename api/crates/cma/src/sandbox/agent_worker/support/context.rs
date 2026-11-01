//! Parent validation and call-local prepared startup observations.
use super::*;
use layer2_core::sandbox::{SandboxStatus, WorkspaceStatus};

impl AgentWorker {
    pub(in crate::sandbox::agent_worker) async fn startup_context(
        &self,
        agent: &Agent,
    ) -> Result<AgentContext, WorkerFailure> {
        if agent.status != AgentStatus::Starting {
            return Err(WorkerFailure::terminal(
                "agent_lifecycle_state_invalid",
                "startup work was scheduled for a non-starting agent",
            ));
        }
        let context = self.context(agent).await?;
        if context.sandbox.status != SandboxStatus::Active
            || !context.sandbox.credential.delivered
            || context.workspace.status != WorkspaceStatus::Ready
            || context.worktree.status != WorktreeStatus::Creating
        {
            return Err(WorkerFailure::retryable(
                "agent_parent_not_ready",
                "the sandbox, workspace, or worktree is not ready for agent startup",
            ));
        }
        Ok(context)
    }

    pub(in crate::sandbox::agent_worker) async fn cleanup_context(
        &self,
        agent: &Agent,
    ) -> Result<AgentContext, WorkerFailure> {
        self.context(agent).await
    }

    pub(in crate::sandbox::agent_worker) async fn context(
        &self,
        agent: &Agent,
    ) -> Result<AgentContext, WorkerFailure> {
        // These IDs come from the claimed Agent. Read independently, then retain
        // the existing error precedence and validate all lineage before adoption.
        let (sandbox, workspace, worktree) = tokio::join!(
            self.dependencies.store.get_sandbox(&agent.sandbox_id),
            self.dependencies.store.get_workspace(&agent.workspace_id),
            self.dependencies.store.get_worktree(&agent.worktree_id),
        );
        let sandbox = sandbox
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        let workspace = workspace
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        let worktree = worktree
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        validate_lineage(agent, &sandbox, &workspace, &worktree)?;
        if sandbox.status == SandboxStatus::Rotating && !sandbox.restored_connection_is_ready(agent)
        {
            return Err(WorkerFailure::retryable(
                "agent_runtime_update_in_progress",
                "the retained runtime update is pending",
            ));
        }
        if sandbox.lifecycle.checkpoint_capture.is_some() {
            return Err(WorkerFailure::retryable(
                "agent_checkpoint_in_progress",
                "checkpoint writer resumption is pending",
            ));
        }
        let inspection_started = std::time::Instant::now();
        let attached = crate::sandbox::runtime_attachment::inspect_bound_runtime(
            self.dependencies.adapter.as_ref(),
            &sandbox,
            &self.config.deployment_id,
        )
        .await;
        tracing::info!(
            agent_id = %agent.id,
            sandbox_id = %sandbox.id,
            step = ?agent.work.step,
            elapsed_ms = inspection_started.elapsed().as_millis() as u64,
            succeeded = attached.is_ok(),
            "Agent runtime inspection completed"
        );
        let (attached, status) = attached.map_err(adapter_failure)?;
        Ok(AgentContext {
            sandbox,
            workspace,
            worktree,
            runtime: attached,
            agent_resource_preparation: status
                .is_some_and(|status| status.agent_resource_preparation),
        })
    }

    pub(in crate::sandbox::agent_worker) async fn prepared_open_context(
        &self,
        agent: &Agent,
        prepared: AgentContext,
    ) -> Result<AgentContext, WorkerFailure> {
        let (sandbox, workspace, worktree) = tokio::join!(
            self.dependencies.store.get_sandbox(&agent.sandbox_id),
            self.dependencies.store.get_workspace(&agent.workspace_id),
            self.dependencies.store.get_worktree(&agent.worktree_id),
        );
        let sandbox = sandbox
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        let workspace = workspace
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        let worktree = worktree
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        if agent.status != AgentStatus::Starting
            || agent.work.step != AgentWorkStep::OpenConversation
            || sandbox != prepared.sandbox
            || workspace != prepared.workspace
            || worktree != prepared.worktree
        {
            return self.startup_context(agent).await;
        }
        validate_lineage(agent, &sandbox, &workspace, &worktree)?;
        // Only the immediate caller that successfully published both replay
        // receipts supplies this observation. The Open transport still proves
        // the exact live Pod and applies the newly claimed runtime fence.
        tracing::info!(agent_id = %agent.id, "Prepared Agent Open retained unchanged parent observation");
        Ok(prepared)
    }
}
