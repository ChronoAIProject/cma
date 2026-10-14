//! Report observed provisioning gates without waiting for the whole Agent.
use super::*;

impl InlineAgentLifecycle {
    pub(super) async fn progress(
        &self,
        lease: &LifecycleLease,
        phase: &str,
        status: &str,
        message: &str,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        self.operations
            .record_progress(lease, phase, status, message)
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)
    }

    pub(super) async fn wait_for_agent_workspace(
        &self,
        caller: &Caller,
        sandbox_id: &SandboxId,
        lease: &LifecycleLease,
    ) -> Result<layer2_core::WorkspaceId, AgentLifecycleProvisioningError> {
        tokio::time::timeout(Duration::from_secs(180), async {
            let mut sandbox_ready = false;
            let mut key_ready = false;
            loop {
                let sandbox = self
                    .sandbox
                    .read(caller, sandbox_id)
                    .await
                    .map_err(sandbox_error)?;
                if matches!(
                    sandbox.status,
                    SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired
                ) {
                    return Err(AgentLifecycleProvisioningError::Conflict(
                        "Sandbox provisioning failed".into(),
                    ));
                }
                if !sandbox_ready
                    && (sandbox.runtime.is_some() || sandbox.status == SandboxStatus::Active)
                {
                    self.progress(lease, "sandbox_ready", "ready", "Agent Sandbox is ready")
                        .await?;
                    self.progress(
                        lease,
                        "key_scope_ready",
                        "running",
                        "Configuring Agent key permissions",
                    )
                    .await?;
                    sandbox_ready = true;
                }
                // Active is also the established readiness gate for retained
                // Sandboxes. Fresh managed Sandboxes publish delivered first.
                if !key_ready
                    && (sandbox.credential.delivered || sandbox.status == SandboxStatus::Active)
                {
                    self.progress(
                        lease,
                        "key_scope_ready",
                        "ready",
                        "Agent key permissions are ready",
                    )
                    .await?;
                    self.progress(
                        lease,
                        "workspace_runtime",
                        "running",
                        "Preparing an empty workspace",
                    )
                    .await?;
                    key_ready = true;
                }
                if sandbox.status == SandboxStatus::Active {
                    let page = self
                        .workspaces
                        .list(caller, sandbox_id, &Default::default())
                        .await
                        .map_err(workspace_error)?;
                    if let Some(workspace) = page
                        .items
                        .into_iter()
                        .find(|workspace| workspace.status == WorkspaceStatus::Ready)
                    {
                        self.progress(lease, "workspace_runtime", "ready", "Workspace is ready")
                            .await?;
                        return Ok(workspace.id);
                    }
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
        .await
        .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?
    }
}
