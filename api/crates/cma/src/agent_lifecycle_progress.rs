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
        expected_workspace: Option<&layer2_core::WorkspaceId>,
    ) -> Result<layer2_core::WorkspaceId, AgentLifecycleProvisioningError> {
        tokio::time::timeout(Duration::from_secs(180), async {
            let started = tokio::time::Instant::now();
            let mut sandbox_ready = false;
            let mut key_ready = false;
            let fallback = tokio::sync::Notify::new();
            let readiness = self
                .native_prepared
                .as_ref()
                .map(|store| store.agent_parent_ready.as_ref())
                .unwrap_or(&fallback);
            loop {
                // Prepared Agents are admitted only after this exact parent wait.
                // Register before authenticated reads; the hint supplies no state.
                let changed = readiness.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if let Some(id) = expected_workspace {
                    let workspace = self
                        .workspaces
                        .read(caller, id)
                        .await
                        .map_err(workspace_error)?;
                    if workspace.status == WorkspaceStatus::Failed {
                        let detail = workspace
                            .failed
                            .filter(|failure| failure.class.starts_with("repository_"))
                            .map(|failure| format!("{}: {}", failure.class, failure.detail))
                            .unwrap_or_else(|| {
                                "Workspace preparation failed. The task has not run.".into()
                            });
                        return Err(AgentLifecycleProvisioningError::Conflict(detail));
                    }
                }
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
                    self.progress_pair(
                        lease,
                        ("sandbox_ready", "ready", "Agent Sandbox is ready"),
                        (
                            "key_scope_ready",
                            "running",
                            "Configuring Agent key permissions",
                        ),
                    )
                    .await?;
                    sandbox_ready = true;
                }
                // Active is also the established readiness gate for retained
                // Sandboxes. Fresh managed Sandboxes publish delivered first.
                if !key_ready
                    && (sandbox.credential.delivered || sandbox.status == SandboxStatus::Active)
                {
                    self.progress_pair(
                        lease,
                        (
                            "key_scope_ready",
                            "ready",
                            "Agent key permissions are ready",
                        ),
                        ("workspace_runtime", "running", "Preparing the workspace"),
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
                    if let Some(workspace) = page.items.into_iter().find(|workspace| {
                        workspace.status == WorkspaceStatus::Ready
                            && expected_workspace.is_none_or(|id| workspace.id == *id)
                    }) {
                        return Ok(workspace.id);
                    }
                }
                tokio::select! {
                    _ = &mut changed => {},
                    _ = tokio::time::sleep(worker::readiness_delay(started)) => {},
                }
            }
        })
        .await
        .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?
    }

    pub(super) async fn progress_pair(
        &self,
        lease: &LifecycleLease,
        first: (&str, &str, &str),
        second: (&str, &str, &str),
    ) -> Result<(), AgentLifecycleProvisioningError> {
        self.operations
            .record_progress_many(lease, &[first, second])
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)
    }
}

#[async_trait]
impl AgentLifecycleOperationReader for InlineAgentLifecycle {
    async fn read(
        &self,
        caller: &Caller,
        operation_id: &str,
    ) -> Result<AgentLifecycleOperationView, AgentLifecycleOperationReadError> {
        let record = self
            .operations
            .read_for_owner(&caller.subject, operation_id)
            .await
            .map_err(|_| AgentLifecycleOperationReadError::Unavailable)?
            .ok_or(AgentLifecycleOperationReadError::NotFound)?;
        let mut view = record.view();
        view.callback_url = callback_url(
            self.agent_records.as_ref(),
            &self.agent_page_origin,
            &record.id,
            record.agent_id.as_deref(),
        )
        .await;
        Ok(view)
    }
}

/// Missing or Deleted Agents yield null; observation errors leave the link unknown.
pub(super) async fn callback_url(
    store: &dyn layer2_core::sandbox::store::AgentStore,
    origin: &layer1_public::agent_page::AgentPageOrigin,
    operation_id: &str,
    agent_id: Option<&str>,
) -> Option<Option<String>> {
    let id = agent_id?;
    match store.get_agent(&AgentId(id.into())).await {
        Ok(agent) => Some(
            agent
                .filter(|agent| agent.status != layer2_core::sandbox::AgentStatus::Deleted)
                .and_then(|agent| origin.agent_url(agent.id.as_str())),
        ),
        Err(_) => {
            tracing::warn!(
                operation_id,
                agent_id = id,
                "could not observe lifecycle Agent page availability"
            );
            None
        }
    }
}
