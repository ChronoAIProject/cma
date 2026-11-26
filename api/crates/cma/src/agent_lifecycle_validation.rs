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

#[path = "agent_lifecycle_validation/setup.rs"]
mod setup;
#[cfg(test)]
pub(super) use setup::first_message;
pub(super) use setup::{
    admitted_first_message, first_message_purpose, public_repository_setup_message, setup_message,
};

pub(super) fn initial_title(request: &AgentLifecycleCreateRequest) -> Option<String> {
    match request.title_mode {
        Some(layer1_public::rest::AgentLifecycleTitleMode::Automatic) => None,
        Some(layer1_public::rest::AgentLifecycleTitleMode::Named) | None => {
            Some(request.name.clone())
        }
    }
}

pub(super) fn validate_request(
    request: &AgentLifecycleCreateRequest,
) -> Result<(), AgentLifecycleProvisioningError> {
    request.validate_template()?;
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
    if let Some(message) = &request.initial_message {
        if message.trim().is_empty()
            || message.len() > layer2_core::sandbox::MAX_FIRST_MESSAGE_BYTES
        {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "initial_message must contain non-whitespace text and at most 262144 UTF-8 bytes"
                    .into(),
            ));
        }
        if request.repository.is_some()
            && (request.workspace_id.is_some() || request.sandbox_id.is_some())
        {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "repository tasks require a new Sandbox; existing Workspace and Sandbox context cannot be replaced".into(),
            ));
        }
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
        if repository.connection_id.as_ref().is_some_and(|id| {
            id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        }) {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "GitHub connection id is invalid".into(),
            ));
        }
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
            || (request.initial_message.is_some()
                && parsed.path().trim_matches('/').split('/').any(|part| {
                    part.is_empty()
                        || matches!(part, "." | ".." | ".git")
                        || !part.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
                        })
                }))
        {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "repository must be an HTTPS GitHub URL".into(),
            ));
        }
        if repository.reference.as_ref().is_some_and(|reference| {
            reference.is_empty()
                || reference.len() > 255
                || reference.chars().any(char::is_control)
                || (request.initial_message.is_some()
                    && (reference.starts_with('-')
                        || reference.chars().any(|character| {
                            character.is_whitespace() || "~^:?*[\\".contains(character)
                        })
                        || reference.contains("..")
                        || reference.contains("@{")))
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
    operation_id: &str,
) -> Result<WorkspaceLocation, AgentLifecycleProvisioningError> {
    if behavior == LifecycleBehavior::PublicRepository {
        let repository = request
            .repository
            .as_ref()
            .ok_or(AgentLifecycleProvisioningError::Unavailable)?;
        let commit = repository
            .reference
            .clone()
            .ok_or(AgentLifecycleProvisioningError::Unavailable)?;
        if layer3_adapter::public_repository_archive_url(&repository.url, &commit).is_none() {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        return Ok(WorkspaceLocation::PublicRepositorySnapshot {
            url: repository.url.clone(),
            commit,
            operation_id: operation_id.into(),
        });
    }
    if let Some(repository) = request
        .repository
        .as_ref()
        .filter(|repository| request.initial_message.is_some() || repository.operation.is_some())
    {
        return Ok(WorkspaceLocation::RepositorySnapshot {
            url: repository.url.clone(),
            reference: repository.reference.clone(),
            connection_id: repository.connection_id.clone(),
        });
    }
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

pub(super) fn sandbox_error(error: SandboxServiceError) -> AgentLifecycleProvisioningError {
    match error {
        SandboxServiceError::Forbidden => AgentLifecycleProvisioningError::Forbidden,
        SandboxServiceError::SandboxLimitReached { limit, live, usage } => {
            AgentLifecycleProvisioningError::SandboxLimitReached {
                limit,
                live: u64::from(live),
                quota_usage: Some(usage),
            }
        }
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

pub(super) fn creation_error(
    error: crate::store::LifecycleCreateError,
) -> AgentLifecycleProvisioningError {
    use crate::store::LifecycleCreateError;
    match error {
        LifecycleCreateError::AdmissionPaused => AgentLifecycleProvisioningError::AdmissionPaused,
        LifecycleCreateError::StaleReleasedRuntime => AgentLifecycleProvisioningError::Unavailable,
        LifecycleCreateError::Store(error) => operation_error(error),
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
