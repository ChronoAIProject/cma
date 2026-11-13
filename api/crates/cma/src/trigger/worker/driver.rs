use std::sync::Arc;

use layer2_core::profiles::{
    content_digest, AgentProfileStore, SandboxProfileStore, WorkspaceProfileStore,
};
use layer2_core::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore};
use layer2_core::sandbox::{
    Agent, AgentService, AgentServiceError, CreateAgent, CreateSandbox, CreateWorkspace, Sandbox,
    SandboxService, SandboxServiceError, Workspace, WorkspaceService, WorkspaceServiceError,
};
use layer2_core::trigger_domain::{
    LaunchContextReader, LaunchEffectKind, ResolvedTriggerProfiles, ServiceConnectionBroker,
    ServiceConnectionError, ServiceRequirement, TriggerId, TriggerRevision, TriggerStore,
    TriggerStoreError, TriggerTarget,
};
use layer2_core::{
    AgentId, Caller, Capability, PlatformSettingsService, SandboxId, ServiceId, SubjectId,
    WorkspaceId,
};

pub(crate) trait LaunchResourceStore:
    SandboxStore + WorkspaceStore + AgentStore + LaunchContextReader
{
}

impl<T> LaunchResourceStore for T where
    T: SandboxStore + WorkspaceStore + AgentStore + LaunchContextReader
{
}

pub(crate) trait LaunchProfileStore:
    SandboxProfileStore + WorkspaceProfileStore + AgentProfileStore
{
}

impl<T> LaunchProfileStore for T where
    T: SandboxProfileStore + WorkspaceProfileStore + AgentProfileStore
{
}

mod effect;
pub(crate) use effect::LaunchResourceEffect;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {safe_detail}")]
pub(crate) struct LaunchResourceError {
    pub(crate) code: &'static str,
    pub(crate) safe_detail: &'static str,
    pub(crate) retryable: bool,
}

impl LaunchResourceError {
    pub(crate) const fn retryable(code: &'static str, safe_detail: &'static str) -> Self {
        Self {
            code,
            safe_detail,
            retryable: true,
        }
    }

    pub(crate) const fn terminal(code: &'static str, safe_detail: &'static str) -> Self {
        Self {
            code,
            safe_detail,
            retryable: false,
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait LaunchResourceDriver: Send + Sync {
    async fn trigger_revision(
        &self,
        trigger_id: &TriggerId,
        digest: &str,
    ) -> Result<Option<TriggerRevision>, LaunchResourceError>;

    async fn frozen_profiles(
        &self,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, LaunchResourceError>;

    async fn orchestration_paused(&self) -> Result<bool, LaunchResourceError>;

    async fn requirements(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, LaunchResourceError>;

    async fn sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, LaunchResourceError>;
    async fn workspace(&self, id: &WorkspaceId) -> Result<Option<Workspace>, LaunchResourceError>;
    async fn agent(&self, id: &AgentId) -> Result<Option<Agent>, LaunchResourceError>;

    async fn matching_workspace(
        &self,
        owner: &SubjectId,
        sandbox_id: &SandboxId,
        profile_id: &layer2_core::WorkspaceProfileId,
    ) -> Result<Option<Workspace>, LaunchResourceError>;

    async fn apply(&self, effect: LaunchResourceEffect) -> Result<String, LaunchResourceError>;
}

pub(crate) struct ServiceLaunchResourceDriver {
    definitions: Arc<dyn TriggerStore>,
    profiles: Arc<dyn LaunchProfileStore>,
    resources: Arc<dyn LaunchResourceStore>,
    settings: Arc<PlatformSettingsService>,
    connections: Arc<dyn ServiceConnectionBroker>,
    sandboxes: Arc<SandboxService>,
    workspaces: Arc<WorkspaceService>,
    agents: Arc<AgentService>,
}

pub(crate) struct ServiceLaunchResourceDependencies {
    pub definitions: Arc<dyn TriggerStore>,
    pub profiles: Arc<dyn LaunchProfileStore>,
    pub resources: Arc<dyn LaunchResourceStore>,
    pub settings: Arc<PlatformSettingsService>,
    pub connections: Arc<dyn ServiceConnectionBroker>,
    pub sandboxes: Arc<SandboxService>,
    pub workspaces: Arc<WorkspaceService>,
    pub agents: Arc<AgentService>,
}

impl ServiceLaunchResourceDriver {
    pub(crate) fn new(dependencies: ServiceLaunchResourceDependencies) -> Arc<Self> {
        Arc::new(Self {
            definitions: dependencies.definitions,
            profiles: dependencies.profiles,
            resources: dependencies.resources,
            settings: dependencies.settings,
            connections: dependencies.connections,
            sandboxes: dependencies.sandboxes,
            workspaces: dependencies.workspaces,
            agents: dependencies.agents,
        })
    }
}

#[async_trait::async_trait]
impl LaunchResourceDriver for ServiceLaunchResourceDriver {
    async fn trigger_revision(
        &self,
        trigger_id: &TriggerId,
        digest: &str,
    ) -> Result<Option<TriggerRevision>, LaunchResourceError> {
        self.definitions
            .trigger_revision(trigger_id, digest)
            .await
            .map_err(trigger_error)
    }

    async fn frozen_profiles(
        &self,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, LaunchResourceError> {
        let (sandbox, workspace, agent) = tokio::try_join!(
            self.profiles.sandbox_profile_revision(
                &target.sandbox_profile.id,
                target.sandbox_profile.revision,
            ),
            self.profiles.workspace_profile_revision(
                &target.workspace_profile.id,
                target.workspace_profile.revision,
            ),
            self.profiles
                .agent_profile_revision(&target.agent_profile.id, target.agent_profile.revision,),
        )
        .map_err(profile_error)?;
        let sandbox = sandbox.ok_or_else(missing_frozen_profile)?;
        let workspace = workspace.ok_or_else(missing_frozen_profile)?;
        let agent = agent.ok_or_else(missing_frozen_profile)?;
        let mut sandbox_workspaces = Vec::with_capacity(sandbox.content.workspace_profiles.len());
        for reference in &sandbox.content.workspace_profiles {
            if reference.id == workspace.profile_id && reference.revision == workspace.revision {
                sandbox_workspaces.push(workspace.clone());
                continue;
            }
            sandbox_workspaces.push(
                self.profiles
                    .workspace_profile_revision(&reference.id, reference.revision)
                    .await
                    .map_err(profile_error)?
                    .ok_or_else(missing_frozen_profile)?,
            );
        }
        validate_profile_digests(&sandbox, &sandbox_workspaces, &workspace, &agent)?;
        Ok(ResolvedTriggerProfiles {
            sandbox,
            sandbox_workspaces,
            workspace,
            agent,
        })
    }

    async fn orchestration_paused(&self) -> Result<bool, LaunchResourceError> {
        self.settings
            .current()
            .await
            .map(|stored| stored.settings.triggers.orchestration_paused)
            .map_err(|_| dependency_unavailable())
    }

    async fn requirements(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, LaunchResourceError> {
        self.connections
            .requirements(owner, services)
            .await
            .map_err(connection_error)
    }

    async fn sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, LaunchResourceError> {
        self.resources.get_sandbox(id).await.map_err(resource_error)
    }

    async fn workspace(&self, id: &WorkspaceId) -> Result<Option<Workspace>, LaunchResourceError> {
        self.resources
            .get_workspace(id)
            .await
            .map_err(resource_error)
    }

    async fn agent(&self, id: &AgentId) -> Result<Option<Agent>, LaunchResourceError> {
        self.resources.get_agent(id).await.map_err(resource_error)
    }

    async fn matching_workspace(
        &self,
        owner: &SubjectId,
        sandbox_id: &SandboxId,
        profile_id: &layer2_core::WorkspaceProfileId,
    ) -> Result<Option<Workspace>, LaunchResourceError> {
        self.resources
            .matching_workspace(owner, sandbox_id, profile_id)
            .await
            .map_err(|error| match error {
                layer2_core::trigger_domain::LaunchResolutionError::Unavailable(_) => {
                    dependency_unavailable()
                }
                layer2_core::trigger_domain::LaunchResolutionError::Corrupt(_) => {
                    resource_corrupt()
                }
            })
    }

    async fn apply(&self, effect: LaunchResourceEffect) -> Result<String, LaunchResourceError> {
        match effect {
            LaunchResourceEffect::CreateSandbox {
                caller,
                authority,
                request,
            } => self
                .sandboxes
                .create_for_launch(&caller, authority, request)
                .await
                .map(|sandbox| sandbox.id.to_string())
                .map_err(sandbox_service_error),
            LaunchResourceEffect::ReviveSandbox {
                caller,
                sandbox_id,
                launch_id,
                reused,
            } => self
                .sandboxes
                .revive_for_launch(&caller, &sandbox_id, &launch_id, reused)
                .await
                .map(|outcome| outcome.sandbox.id.to_string())
                .map_err(sandbox_service_error),
            LaunchResourceEffect::RepairSandboxServiceGrant {
                caller,
                sandbox_id,
                launch_id,
            } => self
                .sandboxes
                .repair_service_grant_for_launch(&caller, &sandbox_id, &launch_id)
                .await
                .map(|outcome| outcome.sandbox.id.to_string())
                .map_err(sandbox_service_error),
            LaunchResourceEffect::CreateWorkspace {
                caller,
                authority,
                sandbox_id,
                request,
                launch_id,
            } => self
                .workspaces
                .create_for_launch(&caller, authority, &sandbox_id, request, launch_id)
                .await
                .map(|workspace| workspace.id.to_string())
                .map_err(workspace_service_error),
            LaunchResourceEffect::CreateAgent {
                caller,
                authority,
                workspace_id,
                request,
                reuse_existing,
                initiation,
            } => self
                .agents
                .create_or_reuse_for_attributed_launch(
                    &caller,
                    authority,
                    &workspace_id,
                    request,
                    reuse_existing,
                    initiation.map(|initiation| *initiation),
                )
                .await
                .map(|agent| agent.id.to_string())
                .map_err(agent_service_error),
        }
    }
}

fn validate_profile_digests(
    sandbox: &layer2_core::profiles::SandboxProfileRevision,
    nested: &[layer2_core::profiles::WorkspaceProfileRevision],
    workspace: &layer2_core::profiles::WorkspaceProfileRevision,
    agent: &layer2_core::profiles::AgentProfileRevision,
) -> Result<(), LaunchResourceError> {
    let sandbox_valid = content_digest(&sandbox.content).ok().as_ref() == Some(&sandbox.digest);
    let nested_valid = nested
        .iter()
        .all(|revision| content_digest(&revision.content).ok().as_ref() == Some(&revision.digest));
    let workspace_valid =
        content_digest(&workspace.content).ok().as_ref() == Some(&workspace.digest);
    let agent_valid = content_digest(&agent.content).ok().as_ref() == Some(&agent.digest);
    if sandbox_valid && nested_valid && workspace_valid && agent_valid {
        Ok(())
    } else {
        Err(resource_corrupt())
    }
}

fn trigger_error(error: TriggerStoreError) -> LaunchResourceError {
    match error {
        TriggerStoreError::Unavailable(_) => dependency_unavailable(),
        _ => resource_corrupt(),
    }
}

fn profile_error(error: layer2_core::profiles::ProfileStoreError) -> LaunchResourceError {
    match error {
        layer2_core::profiles::ProfileStoreError::Unavailable(_) => dependency_unavailable(),
        _ => resource_corrupt(),
    }
}

fn resource_error(error: layer2_core::sandbox::store::SandboxStoreError) -> LaunchResourceError {
    match error {
        layer2_core::sandbox::store::SandboxStoreError::Unavailable(_) => dependency_unavailable(),
        _ => resource_corrupt(),
    }
}

fn connection_error(error: ServiceConnectionError) -> LaunchResourceError {
    match error {
        ServiceConnectionError::Unavailable(_) => dependency_unavailable(),
        ServiceConnectionError::NotFound | ServiceConnectionError::Refused => {
            LaunchResourceError::terminal(
                "service_connection_refused",
                "Service connection status could not be verified",
            )
        }
        ServiceConnectionError::Malformed(_) => resource_corrupt(),
    }
}

fn sandbox_service_error(error: SandboxServiceError) -> LaunchResourceError {
    match error {
        SandboxServiceError::CredentialSetupRequired => LaunchResourceError::terminal(
            "sandbox_credential_setup_required",
            "Sign in to CMA with this same account before starting a new Launch",
        ),
        SandboxServiceError::GlobalCapacityReached => LaunchResourceError::retryable(
            "sandbox_capacity_unavailable",
            "Sandbox capacity is temporarily unavailable; the original Launch will retry",
        ),
        SandboxServiceError::Unavailable(_) => dependency_unavailable(),
        SandboxServiceError::InvalidState => LaunchResourceError::retryable(
            "sandbox_state_changed",
            "Sandbox state changed while launch work was running",
        ),
        SandboxServiceError::SandboxLimitReached { .. } => LaunchResourceError::terminal(
            "sandbox_limit_reached",
            "Sandbox capacity is unavailable for this launch",
        ),
        _ => LaunchResourceError::terminal(
            "sandbox_launch_rejected",
            "Sandbox creation or recovery was rejected",
        ),
    }
}

fn workspace_service_error(error: WorkspaceServiceError) -> LaunchResourceError {
    match error {
        WorkspaceServiceError::Unavailable(_) => dependency_unavailable(),
        WorkspaceServiceError::SandboxNotReady => sandbox_not_ready(),
        WorkspaceServiceError::LimitReached { .. } => LaunchResourceError::terminal(
            "workspace_limit_reached",
            "Workspace capacity is unavailable for this launch",
        ),
        _ => LaunchResourceError::terminal(
            "workspace_launch_rejected",
            "Workspace creation was rejected",
        ),
    }
}

fn agent_service_error(error: AgentServiceError) -> LaunchResourceError {
    match error {
        AgentServiceError::Unavailable(_)
        | AgentServiceError::WorkspaceNotReady
        | AgentServiceError::ControlConflict => dependency_unavailable(),
        AgentServiceError::SandboxNotReady => sandbox_not_ready(),
        AgentServiceError::LimitReached { .. } => LaunchResourceError::terminal(
            "agent_limit_reached",
            "Agent capacity is unavailable for this launch",
        ),
        _ => LaunchResourceError::terminal("agent_launch_rejected", "Agent creation was rejected"),
    }
}

fn sandbox_not_ready() -> LaunchResourceError {
    LaunchResourceError::retryable(
        "launch_sandbox_not_ready",
        "The Sandbox is not ready for launch creation",
    )
}

fn missing_frozen_profile() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "frozen_profile_missing",
        "A profile revision required by this launch is unavailable",
    )
}

fn dependency_unavailable() -> LaunchResourceError {
    LaunchResourceError::retryable(
        "launch_dependency_unavailable",
        "A launch dependency is temporarily unavailable",
    )
}

fn resource_corrupt() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "launch_resource_invalid",
        "Stored launch resource data is not usable",
    )
}

#[cfg(test)]
#[path = "driver_credential_tests.rs"]
mod credential_prerequisite_tests;
