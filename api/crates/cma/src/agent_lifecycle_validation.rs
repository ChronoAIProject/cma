use crate::store::LifecycleBehavior;
use layer1_public::rest::{
    AgentLifecycleCreateRequest, AgentLifecyclePermissions, AgentLifecycleProfile,
    AgentLifecycleProvisioningError,
};
use layer2_core::profiles::{
    AgentProfileContent, PermissionLevel, SkillRef, SkillSource, ThinkingEffort, WorkspaceLocation,
};
use layer2_core::sandbox::{AgentServiceError, SandboxServiceError, WorkspaceServiceError};
use url::Url;

pub(super) fn validate_request(
    request: &AgentLifecycleCreateRequest,
) -> Result<(), AgentLifecycleProvisioningError> {
    if request.name.trim() != request.name
        || request.name.is_empty()
        || request.name.len() > 80
        || request.name.chars().count() > 64
        || request.name.chars().any(char::is_control)
    {
        return Err(AgentLifecycleProvisioningError::Invalid(
            "name must contain 1-80 non-control bytes".into(),
        ));
    }
    if request.workspace_id.is_some() && request.sandbox_id.is_some() {
        return Err(AgentLifecycleProvisioningError::Invalid(
            "choose either sandbox_id or workspace_id".into(),
        ));
    }
    for context in [&request.workspace_id, &request.sandbox_id]
        .into_iter()
        .flatten()
    {
        if context.is_empty()
            || context.len() > 256
            || !context
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || request.image.is_some()
        {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "an existing Sandbox or Workspace requires a valid id and inherits its runtime image".into(),
            ));
        }
    }
    if let Some(image) = &request.image {
        if image.id.trim().is_empty() || image.id.len() > 256 {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "image id is invalid".into(),
            ));
        }
    }
    if let Some(repository) = &request.repository {
        let parsed = Url::parse(&repository.url).map_err(|_| {
            AgentLifecycleProvisioningError::Invalid("repository URL is invalid".into())
        })?;
        if parsed.scheme() != "https"
            || parsed.host_str() != Some("github.com")
            || parsed.username() != ""
            || parsed.password().is_some()
            || parsed.port().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path().trim_matches('/').split('/').count() != 2
        {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "repository must be an HTTPS GitHub URL".into(),
            ));
        }
        if repository.reference.as_ref().is_some_and(|reference| {
            reference.is_empty() || reference.len() > 255 || reference.chars().any(char::is_control)
        }) {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "repository ref is invalid".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn agent_content(
    profile: &AgentLifecycleProfile,
) -> Result<AgentProfileContent, AgentLifecycleProvisioningError> {
    agent_content_for(profile, LifecycleBehavior::PromptRepository)
}

pub(crate) fn agent_content_for(
    profile: &AgentLifecycleProfile,
    behavior: LifecycleBehavior,
) -> Result<AgentProfileContent, AgentLifecycleProvisioningError> {
    let legacy = behavior == LifecycleBehavior::Legacy;
    let thinking_effort =
        match profile
            .reasoning_effort
            .as_deref()
            .unwrap_or(if legacy { "medium" } else { "high" })
        {
            "minimal" => ThinkingEffort::Minimal,
            "low" => ThinkingEffort::Low,
            "medium" => ThinkingEffort::Medium,
            "high" => ThinkingEffort::High,
            "x_high" => ThinkingEffort::XHigh,
            other => {
                return Err(AgentLifecycleProvisioningError::Invalid(format!(
                    "unsupported reasoning_effort {other}"
                )))
            }
        };
    let permission_level = match profile.permission_mode.as_deref().unwrap_or(if legacy {
        "guarded"
    } else {
        "autonomous"
    }) {
        "guarded" => PermissionLevel::Guarded,
        "autonomous" => PermissionLevel::Autonomous,
        other => {
            return Err(AgentLifecycleProvisioningError::Invalid(format!(
                "unsupported permission_mode {other}"
            )))
        }
    };
    let skills = profile
        .skills
        .iter()
        .map(|skill| {
            if skill.id.trim().is_empty() || skill.id.len() > 256 {
                return Err(AgentLifecycleProvisioningError::Invalid(
                    "skill id is invalid".into(),
                ));
            }
            Ok(SkillRef {
                source: SkillSource::Ornn,
                id: layer2_core::SkillId::new(skill.id.clone()),
                version: skill.version.clone(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let value = AgentProfileContent {
        isolated_agent: false,
        model: profile
            .model
            .clone()
            .or_else(|| (!legacy).then(|| "gpt-6".into())),
        thinking_effort,
        permission_level,
        skills,
        system_prompt: profile.system_prompt.clone().unwrap_or_default(),
    };
    value
        .validate()
        .map_err(|error| AgentLifecycleProvisioningError::Invalid(error.to_string()))?;
    Ok(value)
}

pub(super) fn services(
    permissions: &AgentLifecyclePermissions,
) -> Result<Vec<layer2_core::ServiceId>, AgentLifecycleProvisioningError> {
    services_for(permissions, LifecycleBehavior::PromptRepository)
}

pub(super) fn services_for(
    permissions: &AgentLifecyclePermissions,
    behavior: LifecycleBehavior,
) -> Result<Vec<layer2_core::ServiceId>, AgentLifecycleProvisioningError> {
    if permissions.services.len() > 64 {
        return Err(AgentLifecycleProvisioningError::Invalid(
            "at most 64 services may be selected".into(),
        ));
    }
    let mut values = crate::config::REQUIRED_BASELINE_SERVICES
        .iter()
        .filter(|_| behavior != LifecycleBehavior::Legacy)
        .copied()
        .map(layer2_core::ServiceId::new)
        .collect::<Vec<_>>();
    for service in &permissions.services {
        if service.trim().is_empty() || service.len() > 256 || service.chars().any(char::is_control)
        {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "service id is invalid".into(),
            ));
        }
        let service = layer2_core::ServiceId::new(service.clone());
        if behavior == LifecycleBehavior::Legacy || !values.contains(&service) {
            values.push(service);
        }
    }
    Ok(values)
}

pub(super) fn repository_location(
    request: &AgentLifecycleCreateRequest,
    behavior: LifecycleBehavior,
) -> Result<WorkspaceLocation, AgentLifecycleProvisioningError> {
    if behavior == LifecycleBehavior::Legacy {
        if let Some(repository) = &request.repository {
            return Ok(WorkspaceLocation::GitRepository {
                url: repository.url.clone(),
                reference: repository.reference.clone(),
            });
        }
    }
    Ok(WorkspaceLocation::Directory {
        name: "workspace".into(),
    })
}

pub(super) fn first_message(request: &AgentLifecycleCreateRequest) -> String {
    let mut message =
        "You are running in a CMA workspace. Preserve existing files and changes. ".to_string();
    if let Some(repository) = &request.repository {
        // JSON quotation keeps user supplied URL/ref data distinct from instructions.
        let repository = serde_json::to_string(repository).expect("repository is serializable");
        message.push_str(&format!("The user selected this GitHub repository: {repository}. Set up this repository in your current workspace using your available tools. Preserve any existing files and changes. "));
    } else {
        message.push_str("Keep this workspace ready for the user's next task. ");
    }
    message.push_str("If a required NyxID service or GitHub access is missing, call the agent_key_scope_revision MCP tool to request access. Do not ask the user to paste a secret. Wait for CMA to confirm updated access before retrying. Briefly report readiness or the required access.");
    message
}

pub(super) fn sandbox_error(error: SandboxServiceError) -> AgentLifecycleProvisioningError {
    match error {
        SandboxServiceError::Forbidden => AgentLifecycleProvisioningError::Forbidden,
        SandboxServiceError::NameConflict
        | SandboxServiceError::SandboxOriginConflict
        | SandboxServiceError::SandboxLimitReached { .. }
        | SandboxServiceError::NotFound => {
            AgentLifecycleProvisioningError::Conflict(error.to_string())
        }
        SandboxServiceError::ProfileNotFound
        | SandboxServiceError::InvalidName
        | SandboxServiceError::InvalidOrigin
        | SandboxServiceError::TrustConfirmationRequired { .. }
        | SandboxServiceError::InvalidServiceAccess => {
            AgentLifecycleProvisioningError::Invalid(error.to_string())
        }
        _ => AgentLifecycleProvisioningError::Unavailable,
    }
}

pub(super) fn workspace_error(error: WorkspaceServiceError) -> AgentLifecycleProvisioningError {
    match error {
        WorkspaceServiceError::Forbidden | WorkspaceServiceError::NotFound => {
            AgentLifecycleProvisioningError::Forbidden
        }
        WorkspaceServiceError::SandboxNotReady
        | WorkspaceServiceError::NameConflict
        | WorkspaceServiceError::LimitReached { .. }
        | WorkspaceServiceError::OriginConflict => {
            AgentLifecycleProvisioningError::Conflict(error.to_string())
        }
        WorkspaceServiceError::Connection(_) => AgentLifecycleProvisioningError::Unavailable,
        WorkspaceServiceError::ProfileNotFound
        | WorkspaceServiceError::InvalidProfile(_)
        | WorkspaceServiceError::InvalidOrigin => {
            AgentLifecycleProvisioningError::Invalid(error.to_string())
        }
        _ => AgentLifecycleProvisioningError::Unavailable,
    }
}

pub(super) fn connection_error(
    error: layer2_core::sandbox::SandboxCredentialPrerequisiteError,
    target: serde_json::Value,
) -> AgentLifecycleProvisioningError {
    match error {
        layer2_core::sandbox::SandboxCredentialPrerequisiteError::ConnectionsRequired {
            services,
        } => AgentLifecycleProvisioningError::ConnectionRequired {
            services: services
                .into_iter()
                .map(|service| service.to_string())
                .collect(),
            connection_target: target,
        },
        layer2_core::sandbox::SandboxCredentialPrerequisiteError::SetupRequired => {
            AgentLifecycleProvisioningError::ConnectionRequired {
                services: vec!["cma-credentials".into()],
                connection_target: target,
            }
        }
        layer2_core::sandbox::SandboxCredentialPrerequisiteError::Unavailable(_) => {
            AgentLifecycleProvisioningError::Unavailable
        }
    }
}

pub(super) fn agent_error(error: AgentServiceError) -> AgentLifecycleProvisioningError {
    match error {
        AgentServiceError::Forbidden => AgentLifecycleProvisioningError::Forbidden,
        AgentServiceError::InvalidRequest(detail) => {
            AgentLifecycleProvisioningError::Invalid(detail)
        }
        AgentServiceError::CreationConflict
        | AgentServiceError::StatusConflict { .. }
        | AgentServiceError::WorkConflict { .. }
        | AgentServiceError::StopReasonConflict
        | AgentServiceError::ControlConflict => {
            AgentLifecycleProvisioningError::Conflict(error.to_string())
        }
        _ => AgentLifecycleProvisioningError::Unavailable,
    }
}

pub(super) fn operation_error(error: layer2_core::StoreError) -> AgentLifecycleProvisioningError {
    match error {
        layer2_core::StoreError::RevisionConflict { .. } => {
            AgentLifecycleProvisioningError::Conflict(
                "idempotency key was already used for a different lifecycle request".into(),
            )
        }
        _ => AgentLifecycleProvisioningError::Unavailable,
    }
}

pub(super) async fn create_sandbox(
    service: &layer2_core::sandbox::SandboxService,
    caller: &layer2_core::Caller,
    profile: layer2_core::profiles::SandboxProfileRef,
    agent_name: &str,
    operation_id: &str,
) -> Result<layer2_core::sandbox::Sandbox, SandboxServiceError> {
    let mut request = layer2_core::sandbox::CreateSandbox {
        profile,
        name: Some(sandbox_name(agent_name, operation_id)),
        confirmed_digest: None,
        origin: layer2_core::sandbox::SandboxOrigin::Lifecycle {
            operation_id: operation_id.into(),
        },
    };
    match service.create_for_lifecycle(caller, request.clone()).await {
        // A pre-update worker may have committed the old name, then lost the
        // response before persisting its child ID. Replay still verifies the
        // exact owner, operation, profile and historical name in the service.
        Err(SandboxServiceError::SandboxOriginConflict) => {
            request.name = Some(legacy_sandbox_name(agent_name));
            service.create_for_lifecycle(caller, request).await
        }
        result => result,
    }
}

fn legacy_sandbox_name(agent_name: &str) -> String {
    let mut end = agent_name.len().min(56);
    while !agent_name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} sandbox", &agent_name[..end])
}

fn sandbox_name(agent_name: &str, operation_id: &str) -> String {
    // The durable operation's full UUID keeps repeated titles distinct and a
    // lost admission response replayable. Reserve its bytes before truncating
    // the display prefix to the Sandbox's 64-byte name limit.
    let suffix = format!(" sandbox {operation_id}");
    let mut end = agent_name.len().min(64_usize.saturating_sub(suffix.len()));
    while !agent_name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &agent_name[..end])
}

pub(super) fn profile_error(
    error: layer2_core::profiles::ProfileCreationError,
) -> AgentLifecycleProvisioningError {
    use layer2_core::profiles::{store::ProfileStoreError, ProfileCreationError};
    match error {
        ProfileCreationError::Store(ProfileStoreError::Unavailable(_)) => {
            AgentLifecycleProvisioningError::Unavailable
        }
        ProfileCreationError::Forbidden => AgentLifecycleProvisioningError::Forbidden,
        ProfileCreationError::Conflict => {
            AgentLifecycleProvisioningError::Conflict(error.to_string())
        }
        _ => AgentLifecycleProvisioningError::Invalid(error.to_string()),
    }
}

#[cfg(test)]
#[path = "agent_lifecycle_sandbox_name_tests.rs"]
mod sandbox_name_tests;
