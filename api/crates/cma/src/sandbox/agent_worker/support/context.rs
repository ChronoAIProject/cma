//! Parent validation and call-local prepared startup observations.
use super::*;
use layer2_core::agent_ownership::receiver_diagnostics::{with_root, Correlation, TARGET};
use layer2_core::sandbox::{SandboxStatus, WorkspaceStatus};

#[cfg(test)]
#[path = "connection_diagnostics_tests.rs"]
mod diagnostics_tests;
#[path = "context_overlap.rs"]
mod overlap;

fn report_inspection(
    fields: &Correlation,
    step: AgentWorkStep,
    elapsed: std::time::Duration,
    parent_overlap: bool,
    error: Option<&AdapterError>,
) {
    let reason = error.map(|error| match error {
        AdapterError::Unavailable(_) => "runtime_unavailable",
        AdapterError::AccessRevoked => "authority_lost",
        AdapterError::NativeClaimNotReady(_) => "native_claim_not_ready",
        AdapterError::RuntimeCapacity(_) | AdapterError::AtCapacity(_) => "runtime_at_capacity",
        AdapterError::RuntimeContractMismatch(_) => "runtime_contract_mismatch",
        AdapterError::RuntimeStartupHold(_) => "runtime_startup_hold",
        AdapterError::RuntimeLedgerHold(_) => "runtime_ledger_hold",
        _ => "runtime_refused",
    });
    with_root(|| {
        tracing::info!(target: TARGET, parent: None,
            agent_sha256 = %fields.agent_sha256, sandbox_sha256 = fields.sandbox_sha256.as_deref(),
            instance_sha256 = %fields.instance_sha256, receiver_sha256 = fields.receiver_sha256.as_deref(),
            step = ?step, elapsed_ms = elapsed.as_millis() as u64, parent_overlap,
            succeeded = error.is_none(), reason, "Agent runtime inspection completed");
    });
}

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
        self.context_with_parent(agent, None).await
    }

    pub(in crate::sandbox::agent_worker) async fn context_with_parent(
        &self,
        agent: &Agent,
        parent: Option<layer2_core::sandbox::Sandbox>,
    ) -> Result<AgentContext, WorkerFailure> {
        if agent.status == AgentStatus::Starting
            && agent.work.step == AgentWorkStep::OpenConversation
        {
            return self.overlapped_open_context(agent).await;
        }
        // These IDs come from the claimed Agent. Read independently, then retain
        // the existing error precedence and validate all lineage before adoption.
        let (sandbox, workspace, worktree) = tokio::join!(
            async {
                match parent {
                    Some(parent) => Ok(Some(parent)),
                    None => self.dependencies.store.get_sandbox(&agent.sandbox_id).await,
                }
            },
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
        validate_parents(agent, &sandbox, &workspace, &worktree)?;
        let command_generation = if agent.work.step == AgentWorkStep::PublishConnection {
            self.dependencies
                .store
                .agent_command_admission_generation(&agent.id)
                .await
                .map_err(parent_store_failure)?
        } else {
            None
        };
        // This feature only observes parent readiness. It performs no native
        // operation, so a delayed retry cannot conceal an uncertain effect.
        // Capture command and parent revisions before inspection; the existing
        // advisory hint wakes immediately if any of those authorities changes.
        if self.config.parent_not_ready_pacing_enabled
            && agent.work.step == AgentWorkStep::PublishConnection
            && ((sandbox.status != SandboxStatus::Active
                && !sandbox.restored_connection_is_ready(agent))
                || workspace.status != WorkspaceStatus::Ready
                || worktree.status != WorktreeStatus::Ready)
        {
            let mut failure = WorkerFailure::retryable(
                "agent_parent_not_ready",
                "the sandbox, workspace, or worktree is not ready for connection publication",
            );
            failure.observation =
                command_generation.map(|command_admission_generation| AgentRetryObservation {
                    agent_revision: agent.revision,
                    command_admission_generation,
                    sandbox_revision: sandbox.revision,
                    workspace_revision: workspace.revision,
                    worktree_revision: worktree.revision,
                });
            return Err(failure);
        }
        let inspection = self.inspection_correlation(agent);
        let inspection_started = tokio::time::Instant::now();
        let attached = crate::sandbox::runtime_attachment::inspect_bound_runtime(
            self.dependencies.adapter.as_ref(),
            &sandbox,
            &self.config.deployment_id,
        )
        .await;
        self.observe_runtime_startup(&sandbox, inspection_started, &attached);
        report_inspection(
            &inspection,
            agent.work.step,
            inspection_started.elapsed(),
            false,
            attached.as_ref().err(),
        );
        let (attached, status) = attached.map_err(|error| {
            // Inspection is read-only. A timeout here cannot hide an uncertain
            // mutation outcome. Other operation failures retain their cadence.
            let eligible = agent.work.step == AgentWorkStep::PublishConnection
                && matches!(error, AdapterError::Unavailable(_));
            let mut failure = adapter_failure(error);
            if let Some(command_admission_generation) = command_generation.filter(|_| eligible) {
                failure.observation = Some(AgentRetryObservation {
                    agent_revision: agent.revision,
                    command_admission_generation,
                    sandbox_revision: sandbox.revision,
                    workspace_revision: workspace.revision,
                    worktree_revision: worktree.revision,
                });
            }
            failure
        })?;
        Ok(AgentContext {
            sandbox,
            workspace,
            worktree,
            runtime: attached,
            agent_resource_preparation: status
                .is_some_and(|status| status.agent_resource_preparation),
        })
    }

    fn inspection_correlation(&self, agent: &Agent) -> Correlation {
        use layer2_core::agent_ownership::execution_authority;
        if let Some(authority) =
            execution_authority().filter(|authority| authority.token.agent_id == agent.id)
        {
            authority.observe_diagnostic_sandbox(agent.sandbox_id.as_str());
            return authority.receiver_correlation();
        }
        Correlation::new(
            &agent.id,
            Some(agent.sandbox_id.as_str()),
            self.config
                .ownership
                .as_ref()
                .map_or(self.config.holder.as_str(), |owner| {
                    owner.instance_id.as_str()
                }),
        )
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

fn validate_parents(
    agent: &Agent,
    sandbox: &Sandbox,
    workspace: &Workspace,
    worktree: &Worktree,
) -> Result<(), WorkerFailure> {
    validate_lineage(agent, sandbox, workspace, worktree)?;
    if sandbox.status == SandboxStatus::Rotating && !sandbox.restored_connection_is_ready(agent) {
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
    Ok(())
}
