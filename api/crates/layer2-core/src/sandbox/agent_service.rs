mod binding;
mod control;
mod creation;
mod launch;
#[path = "owner_agent_restart_service.rs"]
mod restart;
mod support;
use launch::CreationAttribution;
pub use restart::{AgentRestartControl, AgentRestartObservation, AgentRestartService};
use support::{
    control_error, creation_key, profile_error, require, require_launch, store_error,
    validate_title,
};

use std::sync::Arc;

#[path = "agent_profile_provisioning.rs"]
mod profile_provisioning;

use serde::{Deserialize, Serialize};

use crate::profiles::store::{AgentProfileStore, ProfileStoreError, ProfileUseRecorder};
use crate::profiles::{
    content_digest, AgentProfileRef, AgentProfileRevision, AgentProfileSnapshot,
    MarketplaceLocator, ProfileVisibility,
};
use crate::{
    now_rfc3339, AgentId, Caller, Capability, RuntimeCapabilities, RuntimeCapability, SubjectId,
    WorkspaceId, WorktreeId,
};

use super::service_trust::{validate_agent_profile, validate_agent_revision};
use super::store::{
    AgentAdmission, AgentAdmissionResult, AgentControlStoreError, AgentServiceStore, Page,
    PageRequest, SandboxStoreError,
};
use super::{
    Agent, AgentLogEntry, AgentOrigin, AgentRuntimeError, AgentRuntimeRegistry, AgentStatus,
    AgentWorkStep, FirstMessageRecord, Sandbox, Workspace, WorkspaceStatus,
};

const MAX_TITLE_SCALARS: usize = 64;
const MAX_TRIGGER_KEY_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAgent {
    pub profile: AgentProfileRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_message: Option<String>,
    pub origin: AgentOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentServiceError {
    #[error(transparent)]
    Connection(#[from] super::SandboxCredentialPrerequisiteError),
    #[error("forbidden")]
    Forbidden,
    #[error("agent profile not found")]
    ProfileNotFound,
    #[error("workspace not found")]
    WorkspaceNotFound,
    #[error("agent not found")]
    NotFound,
    #[error("workspace is not ready")]
    WorkspaceNotReady,
    #[error("sandbox is not active")]
    SandboxNotReady,
    #[error("agent runtime does not support {capability:?}")]
    CapabilityUnsupported { capability: RuntimeCapability },
    #[error("sandbox runtime does not support Agent restart")]
    RestartRuntimeUnsupported,
    #[error("the stopped Agent still has an unresolved turn")]
    RestartTurnUnsettled,
    #[error("agent limit reached: {live} of {limit}")]
    LimitReached { limit: u8, live: u8 },
    #[error("agent creation key is already bound to another request")]
    CreationConflict,
    #[error("agent status does not permit this operation: {status:?}")]
    StatusConflict { status: AgentStatus },
    #[error("agent lifecycle work conflicts with this operation: {step:?}")]
    WorkConflict { step: AgentWorkStep },
    #[error("agent stop reason conflicts with the durable request")]
    StopReasonConflict,
    #[error("agent changed while the operation was requested")]
    ControlConflict,
    #[error("agent worktree recovery is not eligible")]
    WorktreeRecoveryConflict,
    #[error("agent log capacity exceeded")]
    LogCapacityExceeded,
    #[error("model budget refused the commitment: {0}")]
    Budget(crate::budget::BudgetError),
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
    #[error("guaranteed model funding is unsupported")]
    ModelFundingUnsupported,
    #[error("invalid agent request: {0}")]
    InvalidRequest(String),
    #[error("agent data is not usable: {0}")]
    Corrupt(String),
    #[error("agent dependency is unavailable: {0}")]
    Unavailable(String),
}

impl AgentServiceError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Connection(error) => error.code(),
            Self::Budget(reason) => reason.code(),
            Self::AdmissionPaused => "admission_paused",
            Self::ModelFundingUnsupported => "model_funding_unsupported",
            Self::Forbidden => "forbidden",
            Self::ProfileNotFound => "agent_profile_not_found",
            Self::WorkspaceNotFound => "workspace_not_found",
            Self::NotFound => "agent_not_found",
            Self::WorkspaceNotReady => "workspace_not_ready",
            Self::SandboxNotReady => "sandbox_not_ready",
            Self::CapabilityUnsupported { .. } => "agent_capability_unsupported",
            Self::RestartRuntimeUnsupported => "agent_restart_runtime_unsupported",
            Self::RestartTurnUnsettled => "agent_restart_turn_unsettled",
            Self::LimitReached { .. } => "agent_limit_reached",
            Self::CreationConflict => "agent_creation_conflict",
            Self::StatusConflict { .. } => "agent_status_conflict",
            Self::WorkConflict { .. } => "agent_work_conflict",
            Self::StopReasonConflict => "agent_stop_reason_conflict",
            Self::ControlConflict => "agent_control_conflict",
            Self::WorktreeRecoveryConflict => "agent_worktree_recovery_conflict",
            Self::LogCapacityExceeded => "agent_log_capacity_exceeded",
            Self::InvalidRequest(_) => "invalid_agent_request",
            Self::Corrupt(_) => "stored_agent_corrupt",
            Self::Unavailable(_) => "agent_service_unavailable",
        }
    }
}

#[derive(Clone)]
pub struct AgentService {
    model_credit_required: bool,
    connections: Arc<dyn super::SandboxCredentialPrerequisite>,
    store: Arc<dyn AgentServiceStore>,
    profiles: Arc<dyn AgentProfileStore>,
    profile_uses: Arc<dyn ProfileUseRecorder>,
    runtimes: Arc<dyn AgentRuntimeRegistry>,
    runtime_capabilities: RuntimeCapabilities,
}

impl AgentService {
    pub fn new<S, P, R>(
        store: Arc<S>,
        profiles: Arc<P>,
        runtimes: Arc<R>,
        runtime_capabilities: RuntimeCapabilities,
    ) -> Arc<Self>
    where
        S: AgentServiceStore + 'static,
        P: AgentProfileStore + ProfileUseRecorder + 'static,
        R: AgentRuntimeRegistry + 'static,
    {
        let agent_profiles: Arc<dyn AgentProfileStore> = profiles.clone();
        let profile_uses: Arc<dyn ProfileUseRecorder> = profiles;
        Self::with_dependencies(
            store,
            agent_profiles,
            profile_uses,
            runtimes,
            runtime_capabilities,
        )
    }

    pub(crate) fn with_dependencies<S, R>(
        store: Arc<S>,
        profiles: Arc<dyn AgentProfileStore>,
        profile_uses: Arc<dyn ProfileUseRecorder>,
        runtimes: Arc<R>,
        runtime_capabilities: RuntimeCapabilities,
    ) -> Arc<Self>
    where
        S: AgentServiceStore + 'static,
        R: AgentRuntimeRegistry + 'static,
    {
        Arc::new(Self {
            model_credit_required: false,
            connections: Arc::new(super::FixedSandboxCredentialPrerequisite::Ready),
            store,
            profiles,
            profile_uses,
            runtimes,
            runtime_capabilities,
        })
    }

    pub fn with_connections(
        &self,
        connections: Arc<dyn super::SandboxCredentialPrerequisite>,
    ) -> Arc<Self> {
        Arc::new(Self {
            model_credit_required: self.model_credit_required,
            connections,
            store: self.store.clone(),
            profiles: self.profiles.clone(),
            profile_uses: self.profile_uses.clone(),
            runtimes: self.runtimes.clone(),
            runtime_capabilities: self.runtime_capabilities.clone(),
        })
    }

    pub fn with_model_credit_required(&self, required: bool) -> Arc<Self> {
        Arc::new(Self {
            model_credit_required: required,
            ..self.clone()
        })
    }

    async fn create_with_profile(
        &self,
        caller: &Caller,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
        frozen_profile: bool,
        reuse_existing: bool,
        attribution: CreationAttribution,
    ) -> Result<Agent, AgentServiceError> {
        self.runtime_capabilities
            .require([RuntimeCapability::Conversation])
            .map_err(|capability| AgentServiceError::CapabilityUnsupported { capability })?;
        validate_title(request.title.as_deref())?;
        let workspace = self.owned_workspace(caller, workspace_id).await?;
        let sandbox = self.parent_sandbox(caller, &workspace).await?;
        let bootstrap = matches!(&request.origin, AgentOrigin::Lifecycle { operation_id }
            if sandbox.accepts_lifecycle_bootstrap(operation_id))
            && super::lifecycle_bootstrap::bootstrap_workspace(&sandbox, &workspace);
        if !bootstrap
            && (workspace.status != WorkspaceStatus::Ready || workspace.resolved_commit.is_none())
        {
            return Err(AgentServiceError::WorkspaceNotReady);
        }
        self.connections.check_sandbox(&sandbox).await?;
        if !bootstrap && !sandbox.accepts_creation_admission() {
            return Err(AgentServiceError::SandboxNotReady);
        }
        let revision = if frozen_profile {
            self.exact_revision(&request.profile).await?
        } else {
            self.read_visible_revision(&caller.subject, &request.profile)
                .await?
        };
        let now = now_rfc3339();
        let agent_id = AgentId::generate();
        let creation_key = creation_key(&request.origin, &agent_id)?;
        let creation_key = if reuse_existing {
            creation_key.replacen("trigger:", "trigger-profile:", 1)
        } else {
            creation_key
        };
        let first_message = request
            .first_message
            .map(|text| {
                let mut record = FirstMessageRecord::new(creation_key.clone(), text, now.clone())?;
                record.purpose = Some(attribution.first_message_purpose);
                record.desktop = attribution.desktop;
                Ok::<_, super::SandboxModelError>(record)
            })
            .transpose()
            .map_err(|error| AgentServiceError::InvalidRequest(error.to_string()))?;
        let admission = AgentAdmission {
            binding: None,
            created_by: attribution.actor,
            owner: caller.subject.clone(),
            workspace_id: workspace.id,
            agent_id,
            worktree_id: WorktreeId::generate(),
            profile: AgentProfileSnapshot::from(&revision),
            capabilities: self.runtime_capabilities.clone(),
            title: request.title,
            first_message,
            origin: request.origin,
            creation_key,
            event_at: now,
        };
        if self.model_credit_required && admission.first_message.is_some() {
            return self
                .store
                .replay_agent_admission(&admission)
                .await
                .map_err(store_error)?
                .map(|result| result.creation().agent.clone())
                .ok_or(AgentServiceError::ModelFundingUnsupported);
        }
        let result = if let Some(initiation) = attribution.initiation {
            self.store
                .admit_catalogue_agent_with_trigger_initiator(admission, initiation, reuse_existing)
                .await
        } else if reuse_existing {
            self.store
                .admit_catalogue_agent_reusing_profile(admission)
                .await
        } else {
            self.store.admit_catalogue_agent(admission).await
        }
        .map_err(store_error)?;
        if let AgentAdmissionResult::Created(creation) = &result {
            self.record_profile_use(&creation.agent).await;
        }
        Ok(result.creation().agent.clone())
    }

    pub async fn read(&self, caller: &Caller, id: &AgentId) -> Result<Agent, AgentServiceError> {
        require(caller)?;
        self.owned_agent(caller, id).await
    }

    pub async fn list(
        &self,
        caller: &Caller,
        page: &PageRequest,
    ) -> Result<Page<Agent>, AgentServiceError> {
        require(caller)?;
        self.store
            .list_agents(&caller.subject, page)
            .await
            .map_err(store_error)
    }

    pub async fn log(
        &self,
        caller: &Caller,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, AgentServiceError> {
        require(caller)?;
        self.owned_agent(caller, id).await?;
        self.store
            .agent_log(id, after, limit)
            .await
            .map_err(store_error)
    }

    pub async fn recover_worktree(
        &self,
        caller: &Caller,
        id: &AgentId,
        operation_id: &str,
    ) -> Result<Agent, AgentServiceError> {
        if !caller.authorization.allows(Capability::ControlAnySandbox) {
            require(caller)?;
        }
        self.store
            .begin_agent_worktree_recovery(
                id,
                caller,
                operation_id,
                &WorktreeId::generate(),
                now_rfc3339(),
            )
            .await
            .map_err(control_error)
    }

    async fn read_visible_revision(
        &self,
        consumer: &SubjectId,
        reference: &AgentProfileRef,
    ) -> Result<AgentProfileRevision, AgentServiceError> {
        let profile = self
            .profiles
            .agent_profile(&reference.id)
            .await
            .map_err(profile_error)?
            .ok_or(AgentServiceError::ProfileNotFound)?;
        validate_agent_profile(reference, &profile).map_err(profile_error)?;
        if profile.retired_at.is_some()
            || (&profile.owner != consumer && profile.visibility != ProfileVisibility::Public)
        {
            return Err(AgentServiceError::ProfileNotFound);
        }
        self.exact_revision(reference).await
    }

    async fn exact_revision(
        &self,
        reference: &AgentProfileRef,
    ) -> Result<AgentProfileRevision, AgentServiceError> {
        let revision = self
            .profiles
            .agent_profile_revision(&reference.id, reference.revision)
            .await
            .map_err(profile_error)?
            .ok_or(AgentServiceError::ProfileNotFound)?;
        validate_agent_revision(reference, &revision).map_err(profile_error)?;
        Ok(revision)
    }

    async fn owned_workspace(
        &self,
        caller: &Caller,
        id: &WorkspaceId,
    ) -> Result<Workspace, AgentServiceError> {
        self.store
            .get_workspace(id)
            .await
            .map_err(store_error)?
            .filter(|workspace| workspace.id == *id && workspace.owner == caller.subject)
            .ok_or(AgentServiceError::WorkspaceNotFound)
    }

    async fn parent_sandbox(
        &self,
        caller: &Caller,
        workspace: &Workspace,
    ) -> Result<Sandbox, AgentServiceError> {
        self.store
            .get_sandbox(&workspace.sandbox_id)
            .await
            .map_err(store_error)?
            .filter(|sandbox| {
                sandbox.id == workspace.sandbox_id
                    && sandbox.owner == caller.subject
                    && sandbox.owner == workspace.owner
            })
            .ok_or_else(|| {
                AgentServiceError::Corrupt("workspace sandbox lineage is missing".into())
            })
    }

    async fn owned_agent(&self, caller: &Caller, id: &AgentId) -> Result<Agent, AgentServiceError> {
        self.store
            .get_agent(id)
            .await
            .map_err(store_error)?
            .filter(|agent| agent.id == *id && agent.owner == caller.subject)
            .ok_or(AgentServiceError::NotFound)
    }

    async fn record_profile_use(&self, agent: &Agent) {
        let locator = MarketplaceLocator::Agent(agent.profile.id.clone());
        if let Err(error) = self.profile_uses.record_profile_use(&locator).await {
            tracing::warn!(
                agent_id = %agent.id,
                profile_id = locator.as_str(),
                %error,
                "agent profile use was not recorded"
            );
        }
    }
}
