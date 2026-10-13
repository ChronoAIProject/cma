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
    AgentOrigin, AgentService, CreateAgent, SandboxService, SandboxServiceError, SandboxStatus,
    WorkspaceService, WorkspaceStatus,
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

use crate::store::{
    CreateResult, LifecycleBehavior, LifecycleInput, LifecycleLease, MongoAgentLifecycleStore,
};

#[path = "agent_lifecycle_profiles.rs"]
mod profiles;

#[path = "agent_lifecycle_sandbox.rs"]
mod existing_sandbox;
#[path = "agent_lifecycle_worker.rs"]
mod worker;
#[path = "agent_lifecycle_workspace.rs"]
mod workspace;

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
    capacity: layer3_adapter::SandboxCapacity,
    operations: Arc<MongoAgentLifecycleStore>,
    self_ref: OnceLock<Weak<Self>>,
    worker_slots: Arc<tokio::sync::Semaphore>,
}

struct ProvisionInput {
    behavior: LifecycleBehavior,
    request: AgentLifecycleCreateRequest,
    runtime_image: Option<String>,
    capacity: layer3_adapter::SandboxCapacity,
    limits: layer2_core::sandbox::SandboxLimits,
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
        capacity: layer3_adapter::SandboxCapacity,
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
            capacity,
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
        mut input: ProvisionInput,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        if input.request.sandbox_id.is_some() {
            self.prepare_existing_sandbox(caller, &mut input).await?;
        }
        if input.request.workspace_id.is_some() {
            return self.provision_in_workspace(caller, &input).await;
        }
        let ProvisionInput {
            behavior,
            request,
            runtime_image,
            capacity,
            limits,
            key,
            lease,
            existing_sandbox_id,
            existing_agent_id,
            actor,
        } = input;
        let (agent_revision, _workspace_profile, sandbox_profile) = profiles::create_profiles(
            self.profiles.as_ref(),
            caller,
            &request,
            runtime_image,
            &key,
            capacity,
            limits,
            behavior,
        )
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
            let sandbox = match validation::create_sandbox(
                self.sandbox.as_ref(),
                caller,
                sandbox_profile.clone(),
                &request.name,
                &lease.id,
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
                        title: Some(request.name.clone()),
                        first_message: (behavior == LifecycleBehavior::PromptRepository)
                            .then(|| validation::first_message(&request)),
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
                "Agent runtime is ready; initial instructions are queued for the Agent",
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
        if request.workspace_id.is_none()
            && request.sandbox_id.is_none()
            && !caller.authorization.allows(Capability::CreateSandbox)
        {
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
        if request.sandbox_id.is_some() {
            self.contextual_sandbox(caller, context.map(|context| &context.actor), &request)
                .await?;
        }
        if request.workspace_id.is_some() {
            self.contextual_workspace(caller, context.map(|context| &context.actor), &request)
                .await?;
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
            behavior: LifecycleBehavior::PromptRepository,
            request,
            resolved_image,
            capacity: Some(self.capacity),
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
