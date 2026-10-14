//! Immutable inline profiles use the capacity captured at operation admission.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn create_profiles(
    profiles: &PrivateProfileCreationService,
    caller: &Caller,
    request: &AgentLifecycleCreateRequest,
    runtime_image: Option<String>,
    key: &str,
    capacity: layer3_adapter::SandboxCapacity,
    limits: layer2_core::sandbox::SandboxLimits,
    behavior: LifecycleBehavior,
) -> Result<
    (AgentProfileRevision, WorkspaceProfileRef, SandboxProfileRef),
    AgentLifecycleProvisioningError,
> {
    let profile = request.profile.clone().unwrap_or_default();
    let agent = validation::agent_content_for(&profile, behavior)?;
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
        location: repository_location(request, behavior)?,
        name: if behavior == LifecycleBehavior::Legacy && request.repository.is_some() {
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
            display_name(&request.name, " workspace"),
            "Inline Agent Manager workspace".into(),
            workspace_content,
        )
        .await
        .map_err(validation::profile_error)?;
    let workspace_ref = WorkspaceProfileRef {
        id: WorkspaceProfileId(workspace_revision.profile_id.to_string()),
        revision: workspace_revision.revision,
    };

    let permissions = validation::services_for(&profile.permissions, behavior)?;
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
            display_name(&request.name, " sandbox"),
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

pub(super) fn display_name(agent_name: &str, suffix: &str) -> String {
    // Internal profile names have their own byte limit. Keep already valid
    // names identical for retained operation replay; the visible Agent title
    // and the operation-derived profile IDs always keep their original values.
    let available = layer2_core::profiles::MAX_PROFILE_NAME_BYTES - suffix.len();
    let mut end = agent_name.len().min(available);
    while !agent_name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &agent_name[..end])
}

#[cfg(test)]
#[path = "agent_lifecycle_profiles_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_lifecycle_profile_name_tests.rs"]
mod name_tests;
