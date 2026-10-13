//! Immutable inline profiles use the capacity captured at operation admission.
use super::*;

pub(super) async fn create_profiles(
    profiles: &PrivateProfileCreationService,
    caller: &Caller,
    request: &AgentLifecycleCreateRequest,
    runtime_image: Option<String>,
    key: &str,
    capacity: layer3_adapter::SandboxCapacity,
    limits: layer2_core::sandbox::SandboxLimits,
) -> Result<
    (AgentProfileRevision, WorkspaceProfileRef, SandboxProfileRef),
    AgentLifecycleProvisioningError,
> {
    let profile = request.profile.clone().unwrap_or_default();
    let agent = agent_content(&profile)?;
    let agent_revision = profiles
        .create_agent_for_owner(
            &caller.subject,
            &format!("agent-manager:{key}:agent"),
            request.name.clone(),
            "Inline Agent Manager configuration".into(),
            agent,
        )
        .await
        .map_err(validation::profile_error)?;

    let workspace_content = WorkspaceProfileContent {
        location: repository_location(request)?,
        name: if request.repository.is_some() {
            String::new()
        } else {
            "workspace".into()
        },
        setup_commands: Vec::new(),
        agent_profiles: Vec::new(),
    };
    let workspace_revision = profiles
        .create_workspace_for_owner(
            &caller.subject,
            &format!("agent-manager:{key}:workspace"),
            format!("{} workspace", request.name),
            "Inline Agent Manager workspace".into(),
            workspace_content,
        )
        .await
        .map_err(validation::profile_error)?;
    let workspace_ref = WorkspaceProfileRef {
        id: WorkspaceProfileId(workspace_revision.profile_id.to_string()),
        revision: workspace_revision.revision,
    };

    let permissions = services(&profile.permissions)?;
    let sandbox_content = SandboxProfileContent {
        runtime_image,
        cpu_millis: capacity.cpu_millis,
        memory_mib: capacity.memory_mib,
        nyxid_services: permissions,
        agent_provider: AgentProvider::Codex,
        model_access: ModelAccess::ChronoaiManaged,
        max_workspaces: limits.max_workspaces,
        max_agents_per_workspace: limits.max_agents_per_workspace,
        workspace_profiles: vec![workspace_ref.clone()],
        software_install_commands: Vec::new(),
        environment_variables: Vec::new(),
    };
    let sandbox_revision = profiles
        .create_sandbox_for_owner(
            &caller.subject,
            &format!("agent-manager:{key}:sandbox"),
            format!("{} sandbox", request.name),
            "Inline Agent Manager sandbox configuration".into(),
            sandbox_content,
        )
        .await
        .map_err(validation::profile_error)?;
    let sandbox_ref = SandboxProfileRef {
        id: SandboxProfileId(sandbox_revision.profile_id.to_string()),
        revision: sandbox_revision.revision,
    };
    Ok((agent_revision, workspace_ref, sandbox_ref))
}

#[cfg(test)]
#[path = "agent_lifecycle_profiles_tests.rs"]
mod tests;
