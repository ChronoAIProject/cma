//! Agent Manager's inline lifecycle provisioning facade.
//!
//! The retained core still stores immutable snapshots for compatibility.  The
//! user-facing facade owns those implementation details: it creates the
//! private revisions, admits a sandbox, waits for the agent-sandbox-backed
//! lifecycle worker to make the workspace ready, and finally admits the Agent.

use async_trait::async_trait;
use layer1_public::rest::{
    AgentLifecycleCreateReceipt, AgentLifecycleCreateRequest, AgentLifecycleProvisioner,
    AgentLifecycleProvisioningError,
};
use layer2_core::bots::{BotOwnerAuthority, BotProfileStore};
use layer2_core::profiles::{
    AgentProfileRevision, AgentProvider, ModelAccess, PrivateProfileCreationService,
    ProfileCapacityBounds, SandboxProfileContent, WorkspaceProfileContent,
};
use layer2_core::profiles::{SandboxProfileRef, WorkspaceProfileRef};
use layer2_core::resource_access::{CallerContext, ResourceOwner};
use layer2_core::sandbox::{
    AgentOrigin, AgentService, CreateAgent, CreateSandbox, SandboxOrigin, SandboxService,
    SandboxServiceError, SandboxStatus, WorkspaceService, WorkspaceStatus,
};
use layer2_core::{Caller, SandboxProfileId, WorkspaceProfileId};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[path = "agent_lifecycle_image.rs"]
mod image;
pub(crate) use image::{ImageManagerResolver, ImageResolver};
#[path = "agent_lifecycle_validation.rs"]
mod validation;
use validation::{
    agent_content, agent_error, repository_location, sandbox_error, services, validate_request,
    workspace_error,
};

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
