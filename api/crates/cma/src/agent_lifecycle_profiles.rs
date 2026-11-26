//! Immutable inline profiles use the capacity captured at operation admission.
use super::*;
use layer2_core::profiles::EnvironmentVariable;

pub(crate) fn prepared_default_agent_profile(
) -> Result<layer2_core::profiles::AgentProfileContent, String> {
    validation::agent_content(&guidance::profile_with_instructions(
        layer1_public::rest::AgentLifecycleProfile::default(),
        Some(guidance::managed_v3()),
    ))
    .map_err(|error| format!("default Agent preparation profile is invalid: {error:?}"))
}

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
    availability: layer2_core::profiles::SandboxProfileAvailability,
    operation_id: &str,
) -> Result<
    (AgentProfileRevision, WorkspaceProfileRef, SandboxProfileRef),
    AgentLifecycleProvisioningError,
> {
    let profile = request.profile.clone().unwrap_or_default();
    let agent = validation::agent_content_for(&profile, behavior)?;
    let canary_operation = crate::store::canary_key::operation_id(key);
    let profile_key = |kind| match canary_operation {
        Some(operation_id) => crate::store::canary_key::profile_key(operation_id, kind),
        None => format!("agent-manager:{key}:{kind}"),
    };
    // The Workspace has no Agent references; only Sandbox publication depends
    // on its revision. Finish both idempotent branches before returning, keeping
    // Agent-first error precedence and leaving partial private profiles replayable.
    let (agent_result, dependent_result) = tokio::join!(
        async {
            profiles
                .create_agent_for_owner(
                    &caller.subject,
                    &profile_key("agent"),
                    request.name.clone(),
                    "Inline Agent Manager configuration".into(),
                    agent,
                )
                .await
                .map_err(validation::profile_error)
        },
        async {
            let workspace_content = WorkspaceProfileContent {
                location: repository_location(request, behavior, operation_id)?,
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
                    &profile_key("workspace"),
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

            let sandbox_content = inline_sandbox_content(
                request,
                runtime_image,
                capacity,
                limits,
                behavior,
                Some(workspace_ref.clone()),
            )?;
            let sandbox_revision = profiles
                .create_sandbox_configured_for_owner(
                    &caller.subject,
                    &profile_key("sandbox"),
                    layer2_core::profiles::PrivateSandboxProfileInput {
                        name: display_name(&request.name, " sandbox"),
                        description: "Inline Agent Manager sandbox configuration".into(),
                        content: sandbox_content,
                        availability,
                    },
                )
                .await
                .map_err(validation::profile_error)?;
            let sandbox_ref = SandboxProfileRef {
                id: SandboxProfileId(sandbox_revision.profile_id.to_string()),
                revision: sandbox_revision.revision,
            };
            Ok::<_, AgentLifecycleProvisioningError>((workspace_ref, sandbox_ref))
        },
    );
    let agent_revision = agent_result?;
    let (workspace_ref, sandbox_ref) = dependent_result?;
    Ok((agent_revision, workspace_ref, sandbox_ref))
}

pub(crate) fn canary_sandbox_content(
    input: &LifecycleInput,
    workspace: WorkspaceProfileRef,
) -> Result<SandboxProfileContent, AgentLifecycleProvisioningError> {
    let capacity = input
        .capacity
        .ok_or(AgentLifecycleProvisioningError::Unavailable)?;
    let request =
        guidance::provisioning_request(&input.request, input.managed_agent_instructions.as_deref());
    inline_sandbox_content(
        &request,
        input.resolved_image.clone(),
        capacity,
        input.captured_limits(),
        input.behavior,
        Some(workspace),
    )
}

pub(super) fn inline_sandbox_content(
    request: &AgentLifecycleCreateRequest,
    runtime_image: Option<String>,
    capacity: layer3_adapter::SandboxCapacity,
    limits: layer2_core::sandbox::SandboxLimits,
    behavior: LifecycleBehavior,
    workspace: Option<WorkspaceProfileRef>,
) -> Result<SandboxProfileContent, AgentLifecycleProvisioningError> {
    let profile = request.profile.clone().unwrap_or_default();
    let mut permissions = validation::services_for(&profile.permissions, behavior)?;
    if behavior != LifecycleBehavior::PublicRepository
        && request.repository.as_ref().is_some_and(|repository| {
            request.initial_message.is_some() || repository.operation.is_some()
        })
        && !permissions
            .iter()
            .any(|service| service.as_str() == "api-github")
    {
        permissions.push(layer2_core::ServiceId::new("api-github"));
    }
    Ok(SandboxProfileContent {
        runtime_image,
        cpu_millis: capacity.cpu_millis,
        memory_mib: capacity.memory_mib,
        nyxid_services: permissions,
        agent_provider: AgentProvider::Codex,
        model_access: ModelAccess::ChronoaiManaged,
        max_workspaces: limits.max_workspaces,
        max_agents_per_workspace: limits.max_agents_per_workspace,
        workspace_profiles: workspace.into_iter().collect(),
        software_install_commands: Vec::new(),
        environment_variables: if request.machine_included {
            vec![EnvironmentVariable {
                name: "CMA_MACHINE_INCLUDED".into(),
                value: "1".into(),
            }]
        } else {
            Vec::new()
        },
    })
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

#[cfg(test)]
#[path = "agent_lifecycle_profile_parallel_tests.rs"]
mod parallel_tests;
