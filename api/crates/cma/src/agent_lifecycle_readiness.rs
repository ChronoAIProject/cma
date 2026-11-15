//! Observe an admitted Workspace without rediscovering its siblings.
use super::*;
use layer2_core::sandbox::{Sandbox, Workspace};
use layer2_core::WorkspaceId;

pub(super) struct Observation {
    pub sandbox: Sandbox,
    workspace: Option<Workspace>,
}

pub(super) async fn observe(
    sandboxes: &SandboxService,
    workspaces: &WorkspaceService,
    caller: &Caller,
    sandbox_id: &SandboxId,
    expected_workspace: Option<&WorkspaceId>,
) -> Result<Observation, AgentLifecycleProvisioningError> {
    let started = tokio::time::Instant::now();
    // Both services authenticate the caller independently. Finish both reads
    // before applying the established Workspace-first failure precedence.
    let (workspace, sandbox) = tokio::join!(
        async {
            match expected_workspace {
                Some(id) => workspaces.read(caller, id).await.map(Some),
                None => Ok(None),
            }
        },
        sandboxes.read(caller, sandbox_id),
    );
    let workspace = workspace.map_err(workspace_error)?;
    if let Some(workspace) = &workspace {
        if workspace.status == WorkspaceStatus::Failed {
            let detail = workspace
                .failed
                .as_ref()
                .filter(|failure| failure.class.starts_with("repository_"))
                .map(|failure| format!("{}: {}", failure.class, failure.detail))
                .unwrap_or_else(|| "Workspace preparation failed. The task has not run.".into());
            return Err(AgentLifecycleProvisioningError::Conflict(detail));
        }
    }
    let sandbox = sandbox.map_err(sandbox_error)?;
    if workspace
        .as_ref()
        .is_some_and(|workspace| workspace.sandbox_id != *sandbox_id)
    {
        return Err(AgentLifecycleProvisioningError::Conflict(
            "the recorded workspace belongs to another Sandbox".into(),
        ));
    }
    if matches!(
        sandbox.status,
        SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired
    ) {
        return Err(AgentLifecycleProvisioningError::Conflict(
            "Sandbox provisioning failed".into(),
        ));
    }
    tracing::debug!(
        %sandbox_id,
        workspace_id = expected_workspace.map(WorkspaceId::as_str),
        elapsed_ms = started.elapsed().as_millis(),
        "observed Agent lifecycle parent readiness"
    );
    Ok(Observation { sandbox, workspace })
}

impl Observation {
    pub(super) async fn ready_workspace(
        self,
        workspaces: &WorkspaceService,
        caller: &Caller,
    ) -> Result<Option<WorkspaceId>, AgentLifecycleProvisioningError> {
        if self.sandbox.status != SandboxStatus::Active {
            return Ok(None);
        }
        if let Some(workspace) = self.workspace {
            // The exact authenticated read is sufficient; a list adds a parent
            // read and can omit this Workspace behind the first page of siblings.
            return Ok((workspace.status == WorkspaceStatus::Ready).then_some(workspace.id));
        }
        let page = workspaces
            .list(caller, &self.sandbox.id, &Default::default())
            .await
            .map_err(workspace_error)?;
        Ok(page
            .items
            .into_iter()
            .find(|workspace| workspace.status == WorkspaceStatus::Ready)
            .map(|workspace| workspace.id))
    }
}

#[cfg(test)]
#[path = "agent_lifecycle_readiness_tests.rs"]
mod tests;
