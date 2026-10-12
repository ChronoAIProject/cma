//! Agent Manager's durable inline lifecycle provisioning facade.
//!
//! HTTP admission is intentionally short. It records the complete request in
//! Mongo, returns an operation id, and lets a leased worker drive the three
//! user-facing gates. The record is reclaimed on process restart.

use async_trait::async_trait;
use layer1_public::rest::{
    AgentLifecycleCreateReceipt, AgentLifecycleCreateRequest, AgentLifecycleOperationReadError,
    AgentLifecycleOperationReader, AgentLifecycleOperationView, AgentLifecycleProvisioner,
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
use layer2_core::{
    AgentId, Authorization, Caller, Capability, SandboxId, SandboxProfileId, SubjectId,
    WorkspaceProfileId,
};
use serde_json::json;
use std::{
    sync::{Arc, OnceLock, Weak},
    time::Duration,
};

use crate::store::{CreateResult, LifecycleInput, LifecycleLease, MongoAgentLifecycleStore};

#[path = "agent_lifecycle_worker.rs"]
mod worker;

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
    operations: Arc<MongoAgentLifecycleStore>,
    self_ref: OnceLock<Weak<Self>>,
    worker_slots: Arc<tokio::sync::Semaphore>,
}

struct ProvisionInput {
    request: AgentLifecycleCreateRequest,
    runtime_image: Option<String>,
    key: String,
    lease: LifecycleLease,
    existing_sandbox_id: Option<String>,
    existing_agent_id: Option<String>,
    actor: Option<layer2_core::resource_access::ResourceActor>,
}

impl InlineAgentLifecycle {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        sandbox: Arc<SandboxService>,
        workspaces: Arc<WorkspaceService>,
        agents: Arc<AgentService>,
        profile_store: Arc<dyn BotProfileStore>,
        image_resolver: Arc<dyn ImageResolver>,
        bounds: ProfileCapacityBounds,
        scanner: Arc<dyn layer2_core::profiles::PublicValueScanner>,
        operations: Arc<MongoAgentLifecycleStore>,
    ) -> Arc<Self> {
        let instance = Arc::new(Self {
            sandbox,
            workspaces,
            agents,
            profiles: PrivateProfileCreationService::new(profile_store, bounds, scanner),
            image_resolver,
            bounds,
            operations,
            self_ref: OnceLock::new(),
            worker_slots: Arc::new(tokio::sync::Semaphore::new(crate::store::DISCOVERY_BATCH)),
        });
        let _ = instance.self_ref.set(Arc::downgrade(&instance));
        instance
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
        request: &AgentLifecycleCreateRequest,
        runtime_image: Option<String>,
        key: &str,
    ) -> Result<
        (AgentProfileRevision, WorkspaceProfileRef, SandboxProfileRef),
        AgentLifecycleProvisioningError,
    > {
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
            .map_err(validation::profile_error)?;
        let workspace_ref = WorkspaceProfileRef {
            id: WorkspaceProfileId(workspace_revision.profile_id.to_string()),
            revision: workspace_revision.revision,
        };

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
            .map_err(validation::profile_error)?;
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
                if matches!(
                    sandbox.status,
                    SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired
                ) {
                    return Err(AgentLifecycleProvisioningError::Conflict(
                        "Sandbox provisioning failed".into(),
                    ));
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

    async fn provision(
        &self,
        caller: &Caller,
        input: ProvisionInput,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        let ProvisionInput {
            request,
            runtime_image,
            key,
            lease,
            existing_sandbox_id,
            existing_agent_id,
            actor,
        } = input;
        let (agent_revision, _workspace_profile, sandbox_profile) = self
            .create_profiles(caller, &request, runtime_image, &key)
            .await?;
        self.operations
            .append_event(
                &lease,
                1,
                "sandbox_ready",
                "running",
                "Creating the Agent Sandbox",
            )
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        let sandbox_id = if let Some(id) = existing_sandbox_id {
            SandboxId(id)
        } else {
            let sandbox = match self
                .sandbox
                .create_for_lifecycle(
                    caller,
                    CreateSandbox {
                        profile: sandbox_profile.clone(),
                        name: Some(validation::sandbox_name(&request.name)),
                        confirmed_digest: None,
                        origin: SandboxOrigin::Lifecycle {
                            operation_id: lease.id.clone(),
                        },
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
            self.operations
                .mark_sandbox(&lease, sandbox.id.as_str())
                .await
                .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
            sandbox.id
        };
        let workspace_id = self.wait_for_agent_workspace(caller, &sandbox_id).await?;
        self.operations
            .append_event(
                &lease,
                2,
                "sandbox_ready",
                "ready",
                "Agent Sandbox is ready",
            )
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        let agent_id = if let Some(id) = existing_agent_id {
            AgentId(id)
        } else {
            let agent = self
                .agents
                .create_for_lifecycle(
                    caller,
                    actor.as_ref(),
                    &workspace_id,
                    CreateAgent {
                        profile: layer2_core::profiles::AgentProfileRef::new(
                            agent_revision.profile_id.clone(),
                            agent_revision.revision,
                        ),
                        title: Some(request.name),
                        first_message: None,
                        origin: AgentOrigin::Lifecycle {
                            operation_id: lease.id.clone(),
                        },
                    },
                )
                .await
                .map_err(|error| match error {
                    layer2_core::sandbox::AgentServiceError::Connection(error) => {
                        validation::connection_error(
                            error,
                            json!({"type":"workspace", "id":workspace_id.as_str()}),
                        )
                    }
                    error => agent_error(error),
                })?;
            self.operations
                .mark_agent(&lease, agent.id.as_str())
                .await
                .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
            agent.id
        };
        self.wait_for_agent_ready(caller, &agent_id).await?;
        self.operations
            .append_event(
                &lease,
                3,
                "workspace_runtime",
                "ready",
                "Workspace, skills, model, reasoning effort, system prompt and permission level are ready",
            )
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        self.operations
            .append_event(
                &lease,
                4,
                "key_scope_ready",
                "ready",
                "Agent key scope configuration is ready",
            )
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        Ok(agent_id.to_string())
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
            .allows(Capability::UseOwnSandboxes)
            .then_some(())
            .ok_or(AgentLifecycleProvisioningError::Forbidden)?;
        if !caller.authorization.allows(Capability::CreateSandbox) {
            return Err(AgentLifecycleProvisioningError::Forbidden);
        }
        let _authority = self.authority(caller, context)?;
        if let Some(record) = self
            .operations
            .replay(&caller.subject, idempotency_key, &request)
            .await
            .map_err(validation::operation_error)?
        {
            self.operations
                .resume_connection(&caller.subject, &record.id)
                .await
                .map_err(validation::operation_error)?;
            self.spawn_worker(record.id.clone());
            return Ok(AgentLifecycleCreateReceipt {
                id: record.id,
                status: "provisioning".into(),
            });
        }
        let profile = request.profile.clone().unwrap_or_default();
        agent_content(&profile)?;
        services(&profile.permissions)?;
        // Resolve the delegated Image Manager reference before persisting the
        // operation. The durable worker then needs no secret after admission.
        let resolved_image = match &request.image {
            Some(image) => Some(
                self.image_resolver
                    .resolve(delegated_credential, image)
                    .await?,
            ),
            None => None,
        };
        let input = LifecycleInput {
            request,
            resolved_image,
            capabilities: caller.authorization.iter().collect(),
            actor: context.map(|context| context.actor.clone()),
        };
        let operation = self
            .operations
            .create(&caller.subject, idempotency_key, input)
            .await
            .map_err(validation::operation_error)?;
        let operation_id = match operation {
            CreateResult::Created(id) => id,
            CreateResult::Existing(record) => record.id.clone(),
        };
        self.spawn_worker(operation_id.clone());
        Ok(AgentLifecycleCreateReceipt {
            id: operation_id,
            status: "provisioning".into(),
        })
    }
}

#[async_trait]
impl AgentLifecycleOperationReader for InlineAgentLifecycle {
    async fn read(
        &self,
        caller: &Caller,
        operation_id: &str,
    ) -> Result<AgentLifecycleOperationView, AgentLifecycleOperationReadError> {
        self.operations
            .read_for_owner(&caller.subject, operation_id)
            .await
            .map_err(|_| AgentLifecycleOperationReadError::Unavailable)?
            .map(|record| record.view())
            .ok_or(AgentLifecycleOperationReadError::NotFound)
    }
}
