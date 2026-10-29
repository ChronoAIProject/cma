//! Reserve the exact lifecycle hierarchy before runtime preparation completes.
use super::*;
use layer2_core::sandbox::{Agent, SandboxOrigin, WorkspaceOrigin, WorkspaceServiceError};
use layer2_core::WorkspaceId;

impl InlineAgentLifecycle {
    pub(super) async fn reserve_bootstrap_workspace(
        &self,
        caller: &Caller,
        sandbox_id: &SandboxId,
        sandbox_profile: &SandboxProfileRef,
        workspace_profile: &WorkspaceProfileRef,
        operation_id: &str,
    ) -> Result<WorkspaceId, AgentLifecycleProvisioningError> {
        let sandbox = self
            .sandbox
            .read(caller, sandbox_id)
            .await
            .map_err(sandbox_error)?;
        if sandbox.origin
            != (SandboxOrigin::Lifecycle {
                operation_id: operation_id.into(),
            })
            || sandbox.profile.id != sandbox_profile.id
            || sandbox.profile.revision != sandbox_profile.revision
            || sandbox.profile.content.workspace_profiles.as_slice()
                != std::slice::from_ref(workspace_profile)
        {
            return Err(AgentLifecycleProvisioningError::Conflict(
                "the recorded Sandbox does not match its lifecycle configuration".into(),
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
        if let Some(id) = self
            .recorded_bootstrap_workspace(caller, sandbox_id, workspace_profile)
            .await?
        {
            return Ok(id);
        }
        // This uses the same profile index as the Sandbox provisioning worker.
        // Its atomic admission returns the existing reservation on replay.
        let result = self
            .workspaces
            .create_profile_workspace(&sandbox, workspace_profile, 0)
            .await;
        match result {
            Ok(workspace) => Ok(workspace.id),
            Err(WorkspaceServiceError::SandboxNotReady) => {
                // The provisioning worker may have admitted the same origin
                // and activated its parent after our initial read.
                self.recorded_bootstrap_workspace(caller, sandbox_id, workspace_profile)
                    .await?
                    .ok_or_else(|| {
                        AgentLifecycleProvisioningError::Conflict(
                            "the Sandbox no longer accepts its startup workspace".into(),
                        )
                    })
            }
            Err(error) => Err(workspace_error(error)),
        }
    }

    async fn recorded_bootstrap_workspace(
        &self,
        caller: &Caller,
        sandbox_id: &SandboxId,
        profile: &WorkspaceProfileRef,
    ) -> Result<Option<WorkspaceId>, AgentLifecycleProvisioningError> {
        let page = self
            .workspaces
            .list(caller, sandbox_id, &Default::default())
            .await
            .map_err(workspace_error)?;
        let mut matches = page
            .items
            .into_iter()
            .filter(|workspace| workspace.origin == (WorkspaceOrigin::SandboxProfile { index: 0 }));
        let Some(workspace) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some()
            || workspace.profile.id != profile.id
            || workspace.profile.revision != profile.revision
            || workspace.sandbox_id != *sandbox_id
        {
            return Err(AgentLifecycleProvisioningError::Conflict(
                "the recorded workspace does not match its lifecycle configuration".into(),
            ));
        }
        Ok(Some(workspace.id))
    }
}

pub(super) fn validate_recorded_agent(
    agent: &Agent,
    sandbox_id: &SandboxId,
    workspace_id: &WorkspaceId,
    operation_id: &str,
) -> Result<(), AgentLifecycleProvisioningError> {
    if agent.sandbox_id != *sandbox_id
        || agent.workspace_id != *workspace_id
        || agent.origin
            != (AgentOrigin::Lifecycle {
                operation_id: operation_id.into(),
            })
    {
        return Err(AgentLifecycleProvisioningError::Conflict(
            "the recorded Agent belongs to another lifecycle context".into(),
        ));
    }
    Ok(())
}
