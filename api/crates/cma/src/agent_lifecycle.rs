//! Agent Manager's inline lifecycle provisioning facade.
//!
//! The retained core still stores immutable snapshots for compatibility.  The
//! user-facing facade owns those implementation details: it creates the
//! private revisions, admits a sandbox, waits for the agent-sandbox-backed
//! lifecycle worker to make the workspace ready, and finally admits the Agent.

use async_trait::async_trait;
use layer1_public::rest::{
    AgentLifecycleCreateReceipt, AgentLifecycleCreateRequest, AgentLifecycleImageReference,
    AgentLifecyclePermissions, AgentLifecycleProfile, AgentLifecycleProvisioner,
    AgentLifecycleProvisioningError,
};
use layer2_core::bots::{BotOwnerAuthority, BotProfileStore};
use layer2_core::profiles::{
    AgentProfileContent, AgentProfileRevision, AgentProvider, ModelAccess, PermissionLevel,
    PrivateProfileCreationService, ProfileCapacityBounds, SandboxProfileContent, SkillRef,
    SkillSource, ThinkingEffort, WorkspaceLocation, WorkspaceProfileContent,
};
use layer2_core::profiles::{SandboxProfileRef, WorkspaceProfileRef};
use layer2_core::resource_access::{CallerContext, ResourceOwner};
use layer2_core::sandbox::{
    AgentOrigin, AgentService, AgentServiceError, CreateAgent, CreateSandbox, SandboxOrigin,
    SandboxService, SandboxServiceError, SandboxStatus, WorkspaceService, WorkspaceServiceError,
    WorkspaceStatus,
};
use layer2_core::{Caller, SandboxProfileId, WorkspaceProfileId};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[async_trait]
pub(crate) trait ImageResolver: Send + Sync {
    async fn resolve(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError>;
}

/// Thin, bounded client for the independently deployed Image Manager. The
/// caller's delegated credential is used for the exact image permission check;
/// CMA never stores or substitutes a service-wide token.
pub(crate) struct ImageManagerResolver {
    base_url: String,
    http: reqwest::Client,
}

impl ImageManagerResolver {
    pub(crate) fn new(base_url: String) -> Result<Arc<Self>, String> {
        let parsed = reqwest::Url::parse(&base_url)
            .map_err(|_| "Image Manager base URL is not a valid URL".to_owned())?;
        if parsed.host_str().is_none()
            || !matches!(parsed.scheme(), "http" | "https")
            || parsed.username() != ""
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || base_url.ends_with('/')
        {
            return Err("Image Manager base URL is not a safe service URL".into());
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Image Manager HTTP client could not be initialized".to_owned())?;
        Ok(Arc::new(Self { base_url, http }))
    }
}

#[derive(serde::Deserialize)]
struct ResolvedImage {
    id: Option<String>,
    reference: Option<String>,
    digest: Option<String>,
    created_at_ms: Option<i64>,
}

#[async_trait]
impl ImageResolver for ImageManagerResolver {
    async fn resolve(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        let credential = delegated_credential.ok_or_else(|| {
            AgentLifecycleProvisioningError::ConnectionRequired {
                services: vec!["cma-image-manager".into()],
                connection_target: json!({"type":"image_manager","image_id":image.id}),
            }
        })?;
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
            segments
                .push("v1")
                .push("images")
                .push(&image.id)
                .push("resolve");
        }
        let version = match image.version.as_deref() {
            Some(value) => value.to_owned(),
            None => self.latest_version(&image.id, credential).await?,
        };
        let response = self
            .http
            .post(url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {credential}"),
            )
            .json(&json!({"version": version.as_str()}))
            .send()
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        if bytes.len() > 64 * 1024 {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AgentLifecycleProvisioningError::ConnectionRequired {
                services: vec!["cma-image-manager".into()],
                connection_target: json!({"type":"image_manager","image_id":image.id,"version":version}),
            });
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "the selected Image Manager image version was not found".into(),
            ));
        }
        if !status.is_success() {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        let resolved: ResolvedImage = serde_json::from_slice(&bytes)
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        let reference = resolved
            .reference
            .ok_or_else(|| AgentLifecycleProvisioningError::Unavailable)?;
        let digest = resolved
            .digest
            .ok_or_else(|| AgentLifecycleProvisioningError::Unavailable)?;
        if !valid_immutable_image(&reference)
            || digest
                != reference
                    .rsplit_once('@')
                    .map(|(_, value)| value)
                    .unwrap_or_default()
        {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        Ok(reference)
    }
}

impl ImageManagerResolver {
    async fn latest_version(
        &self,
        image_id: &str,
        credential: &str,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
            segments
                .push("v1")
                .push("images")
                .push(image_id)
                .push("versions");
        }
        let response = self
            .http
            .get(url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {credential}"),
            )
            .send()
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        if bytes.len() > 64 * 1024 {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AgentLifecycleProvisioningError::ConnectionRequired {
                services: vec!["cma-image-manager".into()],
                connection_target: json!({"type":"image_manager","image_id":image_id}),
            });
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "the selected Image Manager image was not found".into(),
            ));
        }
        if !status.is_success() {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        #[derive(serde::Deserialize)]
        struct Page {
            items: Vec<ResolvedImage>,
        }
        let page: Page = serde_json::from_slice(&bytes)
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        page.items
            .into_iter()
            .filter_map(|value| {
                value
                    .id
                    .map(|id| (value.created_at_ms.unwrap_or_default(), id))
            })
            .max_by(|left, right| left.cmp(right))
            .map(|(_, id)| id)
            .ok_or_else(|| {
                AgentLifecycleProvisioningError::Invalid(
                    "the selected Image Manager image has no successful version".into(),
                )
            })
    }
}

fn valid_immutable_image(value: &str) -> bool {
    let Some((name, digest)) = value.rsplit_once("@sha256:") else {
        return false;
    };
    !name.is_empty()
        && name.len() <= 2_048
        && !name.contains('@')
        && !name.contains("://")
        && !name.starts_with(['.', '-', '_', '/', ':'])
        && !name.ends_with(['.', '-', '_', '/', ':'])
        && !name.contains("//")
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
        })
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) struct InlineAgentLifecycle {
    sandbox: Arc<SandboxService>,
    workspaces: Arc<WorkspaceService>,
    agents: Arc<AgentService>,
    profiles: Arc<PrivateProfileCreationService>,
    image_resolver: Arc<dyn ImageResolver>,
    bounds: ProfileCapacityBounds,
}

impl InlineAgentLifecycle {
    pub(crate) fn new(
        sandbox: Arc<SandboxService>,
        workspaces: Arc<WorkspaceService>,
        agents: Arc<AgentService>,
        profile_store: Arc<dyn BotProfileStore>,
        image_resolver: Arc<dyn ImageResolver>,
        bounds: ProfileCapacityBounds,
        scanner: Arc<dyn layer2_core::profiles::PublicValueScanner>,
    ) -> Arc<Self> {
        Arc::new(Self {
            sandbox,
            workspaces,
            agents,
            profiles: PrivateProfileCreationService::new(profile_store, bounds, scanner),
            image_resolver,
            bounds,
        })
    }

    fn authority(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
    ) -> Result<BotOwnerAuthority, AgentLifecycleProvisioningError> {
        let context = context.ok_or(AgentLifecycleProvisioningError::Forbidden)?;
        if context.caller.subject != caller.subject || !context.validate_principal() {
            return Err(AgentLifecycleProvisioningError::Forbidden);
        }
        BotOwnerAuthority::new(
            context.clone(),
            ResourceOwner::Person {
                subject: caller.subject.clone(),
            },
            None,
        )
        .map_err(|_| AgentLifecycleProvisioningError::Forbidden)
    }

    async fn create_profiles(
        &self,
        caller: &Caller,
        _authority: &BotOwnerAuthority,
        delegated_credential: Option<&str>,
        request: &AgentLifecycleCreateRequest,
        key: &str,
    ) -> Result<
        (AgentProfileRevision, WorkspaceProfileRef, SandboxProfileRef),
        AgentLifecycleProvisioningError,
    > {
        let runtime_image = match &request.image {
            Some(image) => Some(
                self.image_resolver
                    .resolve(delegated_credential, image)
                    .await?,
            ),
            None => None,
        };
        let profile = request.profile.clone().unwrap_or_default();
        let agent = agent_content(&profile)?;
        let agent_revision = self
            .profiles
            .create_agent_for_owner(
                &caller.subject,
                &format!("agent-manager:{key}:agent"),
                request.name.clone(),
                "Inline Agent Manager configuration".into(),
                agent,
            )
            .await
            .map_err(|error| AgentLifecycleProvisioningError::Invalid(error.to_string()))?;

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
        let workspace_revision = self
            .profiles
            .create_workspace_for_owner(
                &caller.subject,
                &format!("agent-manager:{key}:workspace"),
                format!("{} workspace", request.name),
                "Inline Agent Manager workspace".into(),
                workspace_content,
            )
            .await
            .map_err(|error| AgentLifecycleProvisioningError::Invalid(error.to_string()))?;
        let workspace_ref = WorkspaceProfileRef {
            id: WorkspaceProfileId(workspace_revision.profile_id.to_string()),
            revision: workspace_revision.revision,
        };

        // Image selection is intentionally rejected until the Image Manager
        // service-to-service resolver is composed.  Silently falling back to
        // the deployment image would make the UI claim a different image ran.
        if request.image.is_some() {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "selected Image Manager versions are not yet available to the Agent Manager runtime".into(),
            ));
        }
        let permissions = services(&profile.permissions)?;
        let sandbox_content = SandboxProfileContent {
            runtime_image,
            cpu_millis: self.bounds.min_cpu_millis,
            memory_mib: self.bounds.min_memory_mib,
            nyxid_services: permissions,
            agent_provider: AgentProvider::Codex,
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 1,
            max_agents_per_workspace: 1,
            workspace_profiles: vec![workspace_ref.clone()],
            software_install_commands: Vec::new(),
            environment_variables: Vec::new(),
        };
        let sandbox_revision = self
            .profiles
            .create_sandbox_for_owner(
                &caller.subject,
                &format!("agent-manager:{key}:sandbox"),
                format!("{} sandbox", request.name),
                "Inline Agent Manager sandbox configuration".into(),
                sandbox_content,
            )
            .await
            .map_err(|error| AgentLifecycleProvisioningError::Invalid(error.to_string()))?;
        let sandbox_ref = SandboxProfileRef {
            id: SandboxProfileId(sandbox_revision.profile_id.to_string()),
            revision: sandbox_revision.revision,
        };
        Ok((agent_revision, workspace_ref, sandbox_ref))
    }

    async fn wait_for_agent_workspace(
        &self,
        caller: &Caller,
        sandbox_id: &layer2_core::SandboxId,
    ) -> Result<layer2_core::WorkspaceId, AgentLifecycleProvisioningError> {
        tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let sandbox = self
                    .sandbox
                    .read(caller, sandbox_id)
                    .await
                    .map_err(sandbox_error)?;
                if sandbox.status == SandboxStatus::Failed {
                    return Err(AgentLifecycleProvisioningError::Unavailable);
                }
                if sandbox.status == SandboxStatus::Active {
                    let page = self
                        .workspaces
                        .list(caller, sandbox_id, &Default::default())
                        .await
                        .map_err(workspace_error)?;
                    if let Some(workspace) = page
                        .items
                        .into_iter()
                        .find(|workspace| workspace.status == WorkspaceStatus::Ready)
                    {
                        return Ok(workspace.id);
                    }
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
        .await
        .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?
    }
}

#[async_trait]
impl AgentLifecycleProvisioner for InlineAgentLifecycle {
    async fn create(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        delegated_credential: Option<&str>,
        request: AgentLifecycleCreateRequest,
        idempotency_key: &str,
    ) -> Result<AgentLifecycleCreateReceipt, AgentLifecycleProvisioningError> {
        validate_request(&request)?;
        caller
            .authorization
            .allows(layer2_core::Capability::UseOwnSandboxes)
            .then_some(())
            .ok_or(AgentLifecycleProvisioningError::Forbidden)?;
        let authority = self.authority(caller, context)?;
        let (agent_revision, _workspace_profile, sandbox_profile) = self
            .create_profiles(
                caller,
                &authority,
                delegated_credential,
                &request,
                idempotency_key,
            )
            .await?;
        let sandbox = match self
            .sandbox
            .create(
                caller,
                CreateSandbox {
                    profile: sandbox_profile.clone(),
                    name: Some(format!("{} sandbox", request.name)),
                    confirmed_digest: None,
                    origin: SandboxOrigin::Manual,
                },
            )
            .await
        {
            Ok(value) => value,
            Err(SandboxServiceError::ConnectionsRequired { services }) => {
                return Err(AgentLifecycleProvisioningError::ConnectionRequired {
                    services: services
                        .into_iter()
                        .map(|service| service.to_string())
                        .collect(),
                    connection_target: json!({
                        "type":"sandbox_profile",
                        "profile":{"id":sandbox_profile.id.to_string(),"revision":sandbox_profile.revision}
                    }),
                });
            }
            Err(SandboxServiceError::CredentialSetupRequired) => {
                return Err(AgentLifecycleProvisioningError::ConnectionRequired {
                    services: vec!["cma-credentials".into()],
                    connection_target: json!({
                        "type":"sandbox_profile",
                        "profile":{"id":sandbox_profile.id.to_string(),"revision":sandbox_profile.revision}
                    }),
                });
            }
            Err(error) => return Err(sandbox_error(error)),
        };
        let workspace_id = self.wait_for_agent_workspace(caller, &sandbox.id).await?;
        let agent = self
            .agents
            .create(
                caller,
                &workspace_id,
                CreateAgent {
                    profile: layer2_core::profiles::AgentProfileRef::new(
                        agent_revision.profile_id.clone(),
                        agent_revision.revision,
                    ),
                    title: Some(request.name),
                    first_message: None,
                    origin: AgentOrigin::Manual,
                },
            )
            .await
            .map_err(agent_error)?;
        Ok(AgentLifecycleCreateReceipt {
            id: agent.id.to_string(),
            status: "ready",
        })
    }
}

fn validate_request(
    request: &AgentLifecycleCreateRequest,
) -> Result<(), AgentLifecycleProvisioningError> {
    if request.name.trim() != request.name
        || request.name.is_empty()
        || request.name.len() > 80
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
        let parsed = url::Url::parse(&repository.url).map_err(|_| {
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

fn agent_content(
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

fn services(
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

fn repository_location(
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

fn sandbox_error(error: SandboxServiceError) -> AgentLifecycleProvisioningError {
    match error {
        SandboxServiceError::Forbidden => AgentLifecycleProvisioningError::Forbidden,
        SandboxServiceError::ConnectionsRequired { services } => {
            AgentLifecycleProvisioningError::ConnectionRequired {
                services: services
                    .into_iter()
                    .map(|service| service.to_string())
                    .collect(),
                connection_target: json!({"type":"sandbox_profile"}),
            }
        }
        SandboxServiceError::CredentialSetupRequired => {
            AgentLifecycleProvisioningError::ConnectionRequired {
                services: vec!["cma-credentials".into()],
                connection_target: json!({"type":"sandbox_profile"}),
            }
        }
        SandboxServiceError::NameConflict | SandboxServiceError::SandboxOriginConflict => {
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

fn workspace_error(error: WorkspaceServiceError) -> AgentLifecycleProvisioningError {
    match error {
        WorkspaceServiceError::Forbidden => AgentLifecycleProvisioningError::Forbidden,
        WorkspaceServiceError::Connection(error) => connection_error(error),
        WorkspaceServiceError::ProfileNotFound
        | WorkspaceServiceError::InvalidProfile(_)
        | WorkspaceServiceError::InvalidOrigin => {
            AgentLifecycleProvisioningError::Invalid(error.to_string())
        }
        _ => AgentLifecycleProvisioningError::Unavailable,
    }
}

fn connection_error(
    error: layer2_core::sandbox::SandboxCredentialPrerequisiteError,
) -> AgentLifecycleProvisioningError {
    match error {
        layer2_core::sandbox::SandboxCredentialPrerequisiteError::ConnectionsRequired {
            services,
        } => AgentLifecycleProvisioningError::ConnectionRequired {
            services: services
                .into_iter()
                .map(|service| service.to_string())
                .collect(),
            connection_target: json!({"type":"sandbox_profile"}),
        },
        layer2_core::sandbox::SandboxCredentialPrerequisiteError::SetupRequired => {
            AgentLifecycleProvisioningError::ConnectionRequired {
                services: vec!["cma-credentials".into()],
                connection_target: json!({"type":"sandbox_profile"}),
            }
        }
        layer2_core::sandbox::SandboxCredentialPrerequisiteError::Unavailable(_) => {
            AgentLifecycleProvisioningError::Unavailable
        }
    }
}

fn agent_error(error: AgentServiceError) -> AgentLifecycleProvisioningError {
    match error {
        AgentServiceError::Forbidden => AgentLifecycleProvisioningError::Forbidden,
        AgentServiceError::Connection(_) => AgentLifecycleProvisioningError::ConnectionRequired {
            services: vec!["agent-services".into()],
            connection_target: json!({"type":"agent"}),
        },
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
