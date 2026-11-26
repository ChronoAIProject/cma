//! Agent Manager's durable inline lifecycle provisioning facade.
//!
//! HTTP admission is intentionally short. It records the complete request in
//! Mongo, returns an operation id, and lets a leased worker drive the
//! user-facing provisioning stages. The record is reclaimed on process restart.

#[path = "agent_lifecycle_submission.rs"]
mod submission;

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
#[path = "agent_lifecycle_tasks.rs"]
mod tasks;
#[path = "agent_lifecycle_worker.rs"]
mod worker;
pub(crate) use tasks::{DrainTimeout as LifecycleDrainTimeout, TaskScope as LifecycleDrain};
#[path = "agent_lifecycle_workspace.rs"]
mod workspace;

#[cfg(test)]
pub(crate) use workspace::tests::fixture::creation_test_router;

#[path = "agent_lifecycle_admission.rs"]
mod admission;
#[path = "agent_lifecycle_desktop.rs"]
mod desktop;
#[path = "agent_lifecycle_image.rs"]
mod image;
mod repository_preflight;
pub(crate) mod runtime_canary;
#[path = "agent_lifecycle_policy.rs"]
mod task_policy;
pub(crate) use image::{valid_immutable_image, ImageResolver, RetiredImageResolver};
#[path = "agent_lifecycle_validation.rs"]
pub(crate) mod validation;
use validation::{
    agent_content, agent_error, repository_location, sandbox_error, services, validate_request,
    workspace_error,
};

pub(crate) use profiles::{canary_sandbox_content, prepared_default_agent_profile};

pub(crate) struct InlineAgentLifecycle {
    sandbox: Arc<SandboxService>,
    workspaces: Arc<WorkspaceService>,
    agents: Arc<AgentService>,
    agent_records: Arc<dyn layer2_core::sandbox::store::AgentStore>,
    agent_page_origin: layer1_public::agent_page::AgentPageOrigin,
    profiles: Arc<PrivateProfileCreationService>,
    image_resolver: Arc<dyn ImageResolver>,
    bounds: ProfileCapacityBounds,
    capacity: layer3_adapter::SandboxCapacity,
    machine_sandbox_image: Option<String>,
    /// When set, the machine image is this source's release at admission.
    released: OnceLock<Arc<dyn layer3_adapter::ReleasedRuntimeSource>>,
    operations: Arc<MongoAgentLifecycleStore>,
    native_prepared: Option<Arc<crate::store::MongoSandboxStore>>,
    desktop: Option<Arc<layer2_core::sandbox::desktop_creation::DesktopCreationService>>,
    self_ref: OnceLock<Weak<Self>>,
    task_experience: OnceLock<Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>>,
    session_guidance: OnceLock<&'static str>,
    failure_metadata_enabled: OnceLock<bool>,
    repository_preflight: OnceLock<Arc<dyn repository_preflight::RepositoryReadiness>>,
    managed_repository_preflight: OnceLock<bool>,
    managed_repository_broker: OnceLock<Arc<crate::trigger::CmaServiceConnectionBroker>>,
    managed_repository_runtime_images: OnceLock<(String, Vec<String>)>,
    public_repository_images: OnceLock<std::collections::BTreeSet<String>>,
    public_repository_writer: OnceLock<bool>,
    #[cfg(test)]
    public_repository_probe: OnceLock<Arc<dyn repository_preflight::PublicRepositoryReadiness>>,
    worker_slots: Arc<tokio::sync::Semaphore>,
    tasks: LifecycleDrain,
}

struct ProvisionInput {
    creation_fence: Option<layer2_core::sandbox::store::CreationFence>,
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
    canary_profiles: Option<crate::store::CanaryProfiles>,
    repository_access_pin: Option<crate::store::RepositoryAccessPin>,
}

impl InlineAgentLifecycle {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        sandbox: Arc<SandboxService>,
        workspaces: Arc<WorkspaceService>,
        agents: Arc<AgentService>,
        agent_records: Arc<dyn layer2_core::sandbox::store::AgentStore>,
        agent_page_origin: layer1_public::agent_page::AgentPageOrigin,
        profile_store: Arc<dyn BotProfileStore>,
        image_resolver: Arc<dyn ImageResolver>,
        bounds: ProfileCapacityBounds,
        capacity: layer3_adapter::SandboxCapacity,
        machine_sandbox_image: Option<String>,
        scanner: Arc<dyn layer2_core::profiles::PublicValueScanner>,
        operations: Arc<MongoAgentLifecycleStore>,
        native_prepared: Option<Arc<crate::store::MongoSandboxStore>>,
        desktop: Option<Arc<layer2_core::sandbox::desktop_creation::DesktopCreationService>>,
    ) -> Arc<Self> {
        let instance = Arc::new(Self {
            sandbox,
            workspaces,
            agents,
            agent_records,
            agent_page_origin,
            profiles: PrivateProfileCreationService::new(profile_store, bounds, scanner),
            image_resolver,
            bounds,
            capacity,
            machine_sandbox_image,
            released: OnceLock::new(),
            operations,
            native_prepared,
            desktop,
            self_ref: OnceLock::new(),
            task_experience: OnceLock::new(),
            session_guidance: OnceLock::new(),
            failure_metadata_enabled: OnceLock::new(),
            repository_preflight: OnceLock::new(),
            managed_repository_preflight: OnceLock::new(),
            managed_repository_broker: OnceLock::new(),
            managed_repository_runtime_images: OnceLock::new(),
            public_repository_images: OnceLock::new(),
            public_repository_writer: OnceLock::new(),
            #[cfg(test)]
            public_repository_probe: OnceLock::new(),
            worker_slots: Arc::new(tokio::sync::Semaphore::new(crate::store::DISCOVERY_BATCH)),
            tasks: LifecycleDrain::new(crate::store::DISCOVERY_BATCH + 1),
        });
        let _ = instance.self_ref.set(Arc::downgrade(&instance));
        instance
    }

    #[cfg(test)]
    pub(crate) fn with_session_guidance(self: Arc<Self>, v4: bool, v5: bool) -> Arc<Self> {
        self.with_admission_guidance(guidance::admission_instructions(v4, v5))
    }

    pub(crate) fn with_admission_guidance(
        self: Arc<Self>,
        instructions: &'static str,
    ) -> Arc<Self> {
        self.session_guidance
            .set(instructions)
            .expect("session guidance writer is configured once before use");
        self
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
            creation_fence,
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
            canary_profiles,
            repository_access_pin,
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
            &lease.id,
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
        if let Some(expected) = canary_profiles {
            if expected.agent.id != agent_revision.profile_id
                || expected.agent.revision != agent_revision.revision
                || expected.workspace != workspace_profile
                || expected.sandbox != sandbox_profile
            {
                return Err(AgentLifecycleProvisioningError::Conflict(
                    "The canary profiles do not match the admitted operation.".into(),
                ));
            }
        }
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
            let admission_result = admission::create_sandbox_with_fence(
                self.sandbox.as_ref(),
                caller,
                sandbox_profile.clone(),
                &request.name,
                &lease,
                creation_fence,
                repository_access_pin.map(|pin| pin.connection),
            )
            .await;
            tracing::info!(
                operation_id = %lease.id,
                phase = "sandbox_admission",
                elapsed_ms = admission_started.elapsed().as_millis(),
                success = admission_result.is_ok(),
                "inline Agent lifecycle preparation phase completed"
            );
            let sandbox = admission_result
                .map_err(|error| admission::sandbox_admission_error(error, &sandbox_profile))?;
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
        let early_admission = behavior.uses_repository_profile() && !has_prepared_hierarchy;
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
                .create_for_lifecycle_with_preparation(
                    caller,
                    actor.as_ref(),
                    &workspace_id,
                    CreateAgent {
                        profile: layer2_core::profiles::AgentProfileRef::new(
                            agent_revision.profile_id.clone(),
                            agent_revision.revision,
                        ),
                        title: validation::initial_title(&request),
                        first_message,
                        origin: AgentOrigin::Lifecycle {
                            operation_id: lease.id.clone(),
                        },
                    },
                    layer2_core::sandbox::FirstMessagePreparation {
                        purpose: validation::first_message_purpose(&request),
                        desktop: request.desktop,
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
        self.wait_for_prepared_agent(caller, &agent_id, &lease, request.desktop)
            .await?;
        self.progress(&lease, "agent_startup", "ready", "Your Agent is ready")
            .await?;
        Ok(agent_id.to_string())
    }
}

#[async_trait]
impl AgentLifecycleProvisioner for InlineAgentLifecycle {
    async fn desktop_capability(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        scope: layer1_public::rest::DesktopCreationScope,
    ) -> Result<
        layer2_core::sandbox::desktop_creation::DesktopCapability,
        AgentLifecycleProvisioningError,
    > {
        self.read_desktop_capability(caller, context, scope).await
    }

    async fn create(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        delegated_credential: Option<&str>,
        request: AgentLifecycleCreateRequest,
        idempotency_key: &str,
    ) -> Result<AgentLifecycleCreateReceipt, AgentLifecycleProvisioningError> {
        self.submit(
            caller,
            context,
            delegated_credential,
            request,
            idempotency_key,
            None,
        )
        .await
    }

    async fn create_with_submission(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        delegated_credential: Option<&str>,
        request: AgentLifecycleCreateRequest,
        idempotency_key: &str,
        submission: layer1_public::rest::creation_outcomes::Submission,
    ) -> Result<AgentLifecycleCreateReceipt, AgentLifecycleProvisioningError> {
        self.submit(
            caller,
            context,
            delegated_credential,
            request,
            idempotency_key,
            Some(submission),
        )
        .await
    }
}
