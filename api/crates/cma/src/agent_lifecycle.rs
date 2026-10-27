//! Agent Manager's durable inline lifecycle provisioning facade.
//!
//! HTTP admission is intentionally short. It records the complete request in
//! Mongo, returns an operation id, and lets a leased worker drive the
//! user-facing provisioning stages. The record is reclaimed on process restart.

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

#[path = "agent_lifecycle_guidance.rs"]
pub(crate) mod guidance;
#[path = "agent_lifecycle_profiles.rs"]
mod profiles;

#[path = "agent_lifecycle_bootstrap.rs"]
mod bootstrap;
#[path = "agent_lifecycle_sandbox.rs"]
mod existing_sandbox;
#[path = "agent_lifecycle_progress.rs"]
mod progress;
#[path = "agent_lifecycle_worker.rs"]
mod worker;
#[path = "agent_lifecycle_workspace.rs"]
mod workspace;

#[path = "agent_lifecycle_image.rs"]
mod image;
pub(crate) use image::{ImageManagerResolver, ImageResolver};
#[path = "agent_lifecycle_validation.rs"]
pub(crate) mod validation;
use validation::{
    agent_content, agent_error, repository_location, sandbox_error, services, validate_request,
    workspace_error,
};

/// Use the same defaults for explicit inline creation and credential-free preparation.
pub(crate) fn prepared_default_agent_profile(
) -> Result<layer2_core::profiles::AgentProfileContent, String> {
    validation::agent_content(&guidance::profile_with_instructions(
        layer1_public::rest::AgentLifecycleProfile::default(),
        Some(guidance::managed_v3()),
    ))
    .map_err(|error| format!("default Agent preparation profile is invalid: {error:?}"))
}

pub(crate) struct InlineAgentLifecycle {
    sandbox: Arc<SandboxService>,
    workspaces: Arc<WorkspaceService>,
    agents: Arc<AgentService>,
    profiles: Arc<PrivateProfileCreationService>,
    image_resolver: Arc<dyn ImageResolver>,
    bounds: ProfileCapacityBounds,
    capacity: layer3_adapter::SandboxCapacity,
    operations: Arc<MongoAgentLifecycleStore>,
    native_prepared: Option<Arc<crate::store::MongoSandboxStore>>,
    self_ref: OnceLock<Weak<Self>>,
    worker_slots: Arc<tokio::sync::Semaphore>,
}

struct ProvisionInput {
    behavior: LifecycleBehavior,
    first_message: Option<String>,
    request: AgentLifecycleCreateRequest,
    runtime_image: Option<String>,
    capacity: layer3_adapter::SandboxCapacity,
    limits: layer2_core::sandbox::SandboxLimits,
    sandbox_availability: layer2_core::profiles::SandboxProfileAvailability,
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
        native_prepared: Option<Arc<crate::store::MongoSandboxStore>>,
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
            native_prepared,
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
            first_message,
            request,
            runtime_image,
            capacity,
            limits,
            sandbox_availability,
            key,
            lease,
            existing_sandbox_id,
            existing_agent_id,
            actor,
        } = input;
        let profile_started = tokio::time::Instant::now();
        let profile_result = profiles::create_profiles(
            self.profiles.as_ref(),
            caller,
            &request,
            runtime_image,
            &key,
            capacity,
            limits,
            behavior,
            sandbox_availability,
        )
        .await;
        tracing::info!(
            operation_id = %lease.id,
            phase = "profiles",
            elapsed_ms = profile_started.elapsed().as_millis(),
            success = profile_result.is_ok(),
            "inline Agent lifecycle preparation phase completed"
        );
        let (agent_revision, workspace_profile, sandbox_profile) = profile_result?;
        self.progress(
            &lease,
            "sandbox_ready",
            "running",
            "Preparing the Agent Sandbox",
        )
        .await?;
        let sandbox_id = if let Some(id) = existing_sandbox_id {
            SandboxId(id)
        } else {
            let admission_started = tokio::time::Instant::now();
            let admission_result = validation::create_sandbox(
                self.sandbox.as_ref(),
                caller,
                sandbox_profile.clone(),
                &request.name,
                &lease.id,
            )
            .await;
            tracing::info!(
                operation_id = %lease.id,
                phase = "sandbox_admission",
                elapsed_ms = admission_started.elapsed().as_millis(),
                success = admission_result.is_ok(),
                "inline Agent lifecycle preparation phase completed"
            );
            let sandbox = match admission_result {
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
        let has_prepared_hierarchy = match &self.native_prepared {
            Some(store) => store
                .consumed_native_prepared_for_sandbox(&sandbox_id)
                .await
                .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?
                .is_some(),
            None => false,
        };
        // Prepared sessions are adopted only after owner credentials and parent readiness.
        // WaitForWorkspace remains effect-free for ordinary cold admission.
        let early_admission =
            behavior == LifecycleBehavior::PromptRepository && !has_prepared_hierarchy;
        let workspace_id = if early_admission {
            self.reserve_bootstrap_workspace(
                caller,
                &sandbox_id,
                &sandbox_profile,
                &workspace_profile,
                &lease.id,
            )
            .await?
        } else {
            self.wait_for_agent_workspace(caller, &sandbox_id, &lease, None)
                .await?
        };
        let agent_id = if let Some(id) = existing_agent_id {
            let agent = self
                .agents
                .read(caller, &AgentId(id))
                .await
                .map_err(agent_error)?;
            bootstrap::validate_recorded_agent(&agent, &sandbox_id, &workspace_id, &lease.id)?;
            agent.id
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
                        first_message,
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
        // The exact Starting Agent is durable before waiting for runtime I/O.
        // Readiness and startup effects still use the original workspace and
        // the real resolved base commit; early input is only durably queued.
        if early_admission {
            let ready_workspace = self
                .wait_for_agent_workspace(caller, &sandbox_id, &lease, Some(&workspace_id))
                .await?;
            if ready_workspace != workspace_id {
                return Err(AgentLifecycleProvisioningError::Conflict(
                    "the prepared workspace does not match the recorded Agent".into(),
                ));
            }
        }
        self.progress_pair(
            &lease,
            ("workspace_runtime", "ready", "Workspace is ready"),
            ("agent_startup", "running", "Starting your Agent"),
        )
        .await?;
        self.wait_for_agent_ready(caller, &agent_id).await?;
        self.progress(&lease, "agent_startup", "ready", "Your Agent is ready")
            .await?;
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
        agent_content(&guidance::profile_with_instructions(
            profile.clone(),
            Some(guidance::managed_v3()),
        ))?;
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
            managed_agent_instructions: Some(guidance::managed_v3().into()),
            setup_message: Some(validation::setup_message(&request)),
            request,
            resolved_image,
            capacity: Some(self.capacity),
            sandbox_availability: layer2_core::profiles::SandboxProfileAvailability {
                always_keep_alive: true,
                ..Default::default()
            },
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
