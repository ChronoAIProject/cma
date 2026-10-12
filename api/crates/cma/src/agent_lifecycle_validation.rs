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
    let thinking_effort = match profile.reasoning_effort.as_deref().unwrap_or("medium") {
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
    let permission_level = match profile.permission_mode.as_deref().unwrap_or("guarded") {
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
        model: profile.model.clone(),
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
    if permissions.services.len() > 64 {
        return Err(AgentLifecycleProvisioningError::Invalid(
            "at most 64 services may be selected".into(),
        ));
    }
    let mut values = Vec::with_capacity(permissions.services.len());
    for service in &permissions.services {
        if service.trim().is_empty() || service.len() > 256 || service.chars().any(char::is_control)
        {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "service id is invalid".into(),
            ));
        }
        values.push(layer2_core::ServiceId::new(service.clone()));
    }
    Ok(values)
}

pub(super) fn repository_location(
    request: &AgentLifecycleCreateRequest,
) -> Result<WorkspaceLocation, AgentLifecycleProvisioningError> {
    Ok(match &request.repository {
        Some(repository) => WorkspaceLocation::GitRepository {
            url: repository.url.clone(),
            reference: repository.reference.clone(),
        },
        None => WorkspaceLocation::Directory {
            name: "workspace".into(),
        },
    })
}

pub(super) fn sandbox_error(error: SandboxServiceError) -> AgentLifecycleProvisioningError {
    match error {
        SandboxServiceError::Forbidden => AgentLifecycleProvisioningError::Forbidden,
        SandboxServiceError::NameConflict
        | SandboxServiceError::SandboxOriginConflict
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
        WorkspaceServiceError::Forbidden => AgentLifecycleProvisioningError::Forbidden,
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

pub(super) fn sandbox_name(agent_name: &str) -> String {
    let mut end = agent_name.len().min(56);
    while !agent_name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} sandbox", &agent_name[..end])
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
