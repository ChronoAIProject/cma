//! Contextual creation retains the selected Workspace and its Sandbox key scope.
use super::*;
use layer2_core::sandbox::{Sandbox, Workspace};
use layer2_core::WorkspaceId;

impl InlineAgentLifecycle {
    pub(super) async fn contextual_workspace(
        &self,
        caller: &Caller,
        actor: Option<&layer2_core::resource_access::ResourceActor>,
        request: &AgentLifecycleCreateRequest,
    ) -> Result<(Workspace, Sandbox), AgentLifecycleProvisioningError> {
        self.contextual_workspace_for(caller, actor, request, LifecycleBehavior::PromptRepository)
            .await
    }

    async fn contextual_workspace_for(
        &self,
        caller: &Caller,
        actor: Option<&layer2_core::resource_access::ResourceActor>,
        request: &AgentLifecycleCreateRequest,
        behavior: LifecycleBehavior,
    ) -> Result<(Workspace, Sandbox), AgentLifecycleProvisioningError> {
        let id = request.workspace_id.as_ref().ok_or_else(|| {
            AgentLifecycleProvisioningError::Invalid("workspace_id is required".into())
        })?;
        let (workspace, sandbox) = self
            .agents
            .authorize_lifecycle_workspace(caller, actor, &WorkspaceId(id.clone()))
            .await
            .map_err(|error| contextual_error(error, id))?;
        validate_scopes_for(request, &sandbox.credential.effective_reaches(), behavior)?;
        Ok((workspace, sandbox))
    }

    pub(super) async fn provision_in_workspace(
        &self,
        caller: &Caller,
        input: &ProvisionInput,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        // Repeat authorization after every restart, before profile mutation.
        let (workspace, sandbox) = self
            .contextual_workspace_for(caller, input.actor.as_ref(), &input.request, input.behavior)
            .await?;
        if input
            .existing_sandbox_id
            .as_deref()
            .is_some_and(|id| id != sandbox.id.as_str())
        {
            return Err(AgentLifecycleProvisioningError::Conflict(
                "the selected workspace changed its Sandbox".into(),
            ));
        }
        self.operations
            .mark_sandbox(&input.lease, sandbox.id.as_str())
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        self.progress(
            &input.lease,
            "sandbox_ready",
            "running",
            "Checking the selected Sandbox",
        )
        .await?;
        self.progress(
            &input.lease,
            "sandbox_ready",
            "ready",
            "Selected Sandbox is ready",
        )
        .await?;
        self.progress(
            &input.lease,
            "workspace_runtime",
            "running",
            "Checking the selected workspace",
        )
        .await?;
        self.progress(
            &input.lease,
            "workspace_runtime",
            "ready",
            "Selected workspace is ready",
        )
        .await?;
        self.progress(
            &input.lease,
            "key_scope_ready",
            "running",
            "Checking Agent key permissions",
        )
        .await?;
        self.progress(
            &input.lease,
            "key_scope_ready",
            "ready",
            "Agent inherits the existing Sandbox key permissions",
        )
        .await?;
        self.progress(
            &input.lease,
            "agent_startup",
            "running",
            "Starting your Agent",
        )
        .await?;
        let agent_id = if let Some(id) = &input.existing_agent_id {
            let agent = self
                .agents
                .read(caller, &AgentId(id.clone()))
                .await
                .map_err(agent_error)?;
            if agent.workspace_id != workspace.id
                || agent.sandbox_id != sandbox.id
                || agent.origin
                    != (AgentOrigin::Lifecycle {
                        operation_id: input.lease.id.clone(),
                    })
            {
                return Err(AgentLifecycleProvisioningError::Conflict(
                    "the recorded Agent belongs to another lifecycle context".into(),
                ));
            }
            agent.id
        } else {
            let profile = validation::agent_content_for(
                &input.request.profile.clone().unwrap_or_default(),
                input.behavior,
            )?;
            let revision = self
                .profiles
                .create_agent_for_owner(
                    &caller.subject,
                    &format!("agent-manager:{}:agent", input.key),
                    input.request.name.clone(),
                    "Inline Agent Manager configuration".into(),
                    profile,
                )
                .await
                .map_err(validation::profile_error)?;
            // AgentService checks current ownership/readiness again and its
            // transactional admission preserves one Agent/worktree per origin.
            let agent = self
                .agents
                .create_for_lifecycle_with_preparation(
                    caller,
                    input.actor.as_ref(),
                    &workspace.id,
                    CreateAgent {
                        profile: layer2_core::profiles::AgentProfileRef::new(
                            revision.profile_id,
                            revision.revision,
                        ),
                        title: Some(input.request.name.clone()),
                        first_message: input.first_message.clone(),
                        origin: AgentOrigin::Lifecycle {
                            operation_id: input.lease.id.clone(),
                        },
                    },
                    layer2_core::sandbox::FirstMessagePreparation {
                        purpose: validation::first_message_purpose(&input.request),
                        desktop: input.request.desktop,
                    },
                )
                .await
                .map_err(|error| contextual_error(error, workspace.id.as_str()))?;
            self.operations
                .mark_agent(&input.lease, agent.id.as_str())
                .await
                .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
            agent.id
        };
        self.wait_for_prepared_agent(caller, &agent_id, &input.lease, input.request.desktop)
            .await?;
        self.progress(
            &input.lease,
            "agent_startup",
            "ready",
            "Your Agent is ready",
        )
        .await?;
        Ok(agent_id.to_string())
    }
}

fn contextual_error(
    error: layer2_core::sandbox::AgentServiceError,
    workspace: &str,
) -> AgentLifecycleProvisioningError {
    match error {
        layer2_core::sandbox::AgentServiceError::WorkspaceNotFound
        | layer2_core::sandbox::AgentServiceError::NotFound => {
            AgentLifecycleProvisioningError::Forbidden
        }
        layer2_core::sandbox::AgentServiceError::WorkspaceNotReady
        | layer2_core::sandbox::AgentServiceError::SandboxNotReady
        | layer2_core::sandbox::AgentServiceError::LimitReached { .. } => {
            AgentLifecycleProvisioningError::Conflict(error.to_string())
        }
        layer2_core::sandbox::AgentServiceError::Connection(error) => {
            validation::connection_error(error, json!({"type":"workspace","id":workspace}))
        }
        error => agent_error(error),
    }
}

pub(super) fn validate_scopes(
    request: &AgentLifecycleCreateRequest,
    inherited: &[layer2_core::ServiceId],
) -> Result<(), AgentLifecycleProvisioningError> {
    validate_scopes_for(request, inherited, LifecycleBehavior::PromptRepository)
}

fn validate_scopes_for(
    request: &AgentLifecycleCreateRequest,
    inherited: &[layer2_core::ServiceId],
    behavior: LifecycleBehavior,
) -> Result<(), AgentLifecycleProvisioningError> {
    let requested = validation::services_for(
        &request.profile.clone().unwrap_or_default().permissions,
        behavior,
    )?;
    if requested.iter().any(|service| !inherited.contains(service)) {
        return Err(AgentLifecycleProvisioningError::Invalid("an Agent in an existing Workspace inherits its Sandbox key scope; additional services require updating that Sandbox first".into()));
    }
    Ok(())
}

#[cfg(test)]
#[path = "agent_lifecycle_workspace_tests.rs"]
mod tests;
