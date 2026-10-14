//! Transfer only actual, durable preparation receipts after one fresh native observation.
use super::*;
use layer2_core::sandbox::WorkspaceOrigin;
use layer3_adapter::{AdapterError, RuntimeInspection};

impl WorkspaceWorker {
    pub(super) async fn transfer_prepared(
        &self,
        workspace: &Workspace,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        if workspace.origin != (WorkspaceOrigin::SandboxProfile { index: 0 }) {
            return Ok(false);
        }
        let Some(sandbox) = self
            .dependencies
            .store
            .get_sandbox(&workspace.sandbox_id)
            .await?
        else {
            return Ok(false);
        };
        let Some(completed) = self
            .dependencies
            .store
            .prepared_workspace_completion(&sandbox, workspace)
            .await?
        else {
            return Ok(false);
        };
        self.complete_prepared(workspace, &sandbox, completed, now)
            .await?;
        Ok(true)
    }

    pub(in crate::sandbox) async fn complete_prepared(
        &self,
        workspace: &Workspace,
        sandbox: &Sandbox,
        completed: layer2_core::sandbox::store::WorkspacePreparationCompletion,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let result = self.inspect_prepared(sandbox).await;
        if let Err(error) = result {
            self.handle_failure(workspace.clone(), failure(error), now)
                .await?;
            return Ok(());
        }
        let execution = workspace.work.execution.ok_or_else(|| {
            SandboxStoreError::Corrupt("prepared Workspace execution missing".into())
        })?;
        let prepare = WorkspaceRequest::with_clone_depth(
            workspace.operation_id(WorkspaceWorkStep::Prepare),
            workspace.id.to_string(),
            workspace.name.clone(),
            RuntimeWorkspaceSource::Init,
            execution.clone_depth,
        );
        let setup = WorkspaceSetupRequest::with_timeouts(
            workspace.operation_id(WorkspaceWorkStep::Setup),
            workspace.id.to_string(),
            workspace.name.clone(),
            workspace.profile.content.preparation_commands(),
            execution.command_timeout_seconds,
            execution.total_timeout_seconds,
            layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        );
        if workspace.source != WorkspaceSource::Empty
            || !setup.commands.is_empty()
            || !workspace.profile.content.credential_commands().is_empty()
            || !valid_prepare_receipt(&prepare, &completed.prepare, &workspace.path)
            || !valid_setup_receipt(&setup, &completed.setup)
        {
            self.handle_failure(workspace.clone(), WorkerFailure::invalid_receipt(), now)
                .await?;
            return Ok(());
        }
        let mut next = workspace.clone();
        next.record_resolved_commit(completed.prepare.resolved_commit)
            .map_err(|_| SandboxStoreError::Corrupt("prepared Workspace commit invalid".into()))?;
        // The existing fenced transaction publishes WorkspaceReady and wakes blocked Agents.
        self.mark_ready(next, now).await?;
        Ok(())
    }

    async fn inspect_prepared(&self, sandbox: &Sandbox) -> layer3_adapter::Result<()> {
        let binding = sandbox
            .runtime
            .as_ref()
            .ok_or_else(|| AdapterError::Rejected("prepared runtime missing".into()))?;
        let request = super::super::runtime_attachment::retained_runtime_request(
            sandbox,
            binding,
            &self.config.deployment_id,
        )?;
        let started = std::time::Instant::now();
        let result = self.dependencies.adapter.inspect_runtime(&request).await;
        tracing::info!(sandbox_id = %sandbox.id, elapsed_ms = started.elapsed().as_millis() as u64, succeeded = result.is_ok(), "Prepared Workspace runtime inspection completed");
        let inspected =
            result?.ok_or_else(|| AdapterError::Unavailable("prepared runtime absent".into()))?;
        validate_inspection(sandbox, &inspected)
    }
}

fn validate_inspection(
    sandbox: &Sandbox,
    inspected: &RuntimeInspection,
) -> layer3_adapter::Result<()> {
    let binding = sandbox
        .runtime
        .as_ref()
        .ok_or_else(|| AdapterError::Rejected("prepared runtime missing".into()))?;
    let status = inspected
        .status
        .as_ref()
        .ok_or_else(|| AdapterError::Rejected("prepared runtime status missing".into()))?;
    if inspected.observation.handle.as_str() != binding.runtime_id
        || inspected.observation.sandbox_id != sandbox.id.as_str()
        || Some(inspected.observation.generation) != binding.provision_generation
        || !inspected.observation.lifecycle.is_running()
        || binding.boot_id.is_none()
        || !sandbox
            .runtime_process_matches(status)
            .map_err(|_| AdapterError::Rejected("prepared runtime identity invalid".into()))?
        || !status.installation_ready
        || !status.credential_ready
        || status.restore_pending
    {
        return Err(AdapterError::Unavailable(
            "prepared Workspace runtime is no longer ready".into(),
        ));
    }
    Ok(())
}
