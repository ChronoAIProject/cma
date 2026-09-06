use std::sync::Arc;

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
    AgentAdmission, AgentAdmissionResult, AgentControlStoreError, AgentControlTarget,
    AgentDeleteRequest, AgentServiceStore, AgentStopRequest, Page, PageRequest, SandboxStoreError,
};
use super::{
    Agent, AgentLogEntry, AgentOrigin, AgentRuntimeError, AgentRuntimeRegistry, AgentStatus,
    AgentStopReason, AgentWorkStep, FirstMessageRecord, Sandbox, Workspace, WorkspaceStatus,
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
    #[error("agent log capacity exceeded")]
    LogCapacityExceeded,
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
            Self::Forbidden => "forbidden",
            Self::ProfileNotFound => "agent_profile_not_found",
            Self::WorkspaceNotFound => "workspace_not_found",
            Self::NotFound => "agent_not_found",
            Self::WorkspaceNotReady => "workspace_not_ready",
            Self::SandboxNotReady => "sandbox_not_ready",
            Self::CapabilityUnsupported { .. } => "agent_capability_unsupported",
            Self::LimitReached { .. } => "agent_limit_reached",
            Self::CreationConflict => "agent_creation_conflict",
            Self::StatusConflict { .. } => "agent_status_conflict",
            Self::WorkConflict { .. } => "agent_work_conflict",
            Self::StopReasonConflict => "agent_stop_reason_conflict",
            Self::ControlConflict => "agent_control_conflict",
            Self::LogCapacityExceeded => "agent_log_capacity_exceeded",
            Self::InvalidRequest(_) => "invalid_agent_request",
            Self::Corrupt(_) => "stored_agent_corrupt",
            Self::Unavailable(_) => "agent_service_unavailable",
        }
    }
}

pub struct AgentService {
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
            store,
            profiles,
            profile_uses,
            runtimes,
            runtime_capabilities,
        })
    }

    pub async fn create(
        &self,
        caller: &Caller,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
    ) -> Result<Agent, AgentServiceError> {
        require(caller)?;
        if !matches!(&request.origin, AgentOrigin::Manual) {
            return Err(AgentServiceError::InvalidRequest(
                "trigger origin requires launch admission".into(),
            ));
        }
        self.create_with_profile(caller, workspace_id, request, false)
            .await
    }

    pub async fn create_for_launch(
        &self,
        caller: &Caller,
        authority: Capability,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
    ) -> Result<Agent, AgentServiceError> {
        require_launch(caller, authority)?;
        if !matches!(&request.origin, AgentOrigin::Trigger { .. }) {
            return Err(AgentServiceError::InvalidRequest(
                "launch admission requires trigger origin".into(),
            ));
        }
        self.create_with_profile(caller, workspace_id, request, true)
            .await
    }

    async fn create_with_profile(
        &self,
        caller: &Caller,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
        frozen_profile: bool,
    ) -> Result<Agent, AgentServiceError> {
        self.runtime_capabilities
            .require([RuntimeCapability::Conversation])
            .map_err(|capability| AgentServiceError::CapabilityUnsupported { capability })?;
        validate_title(request.title.as_deref())?;
        let workspace = self.owned_workspace(caller, workspace_id).await?;
        if workspace.status != WorkspaceStatus::Ready || workspace.resolved_commit.is_none() {
            return Err(AgentServiceError::WorkspaceNotReady);
        }
        let sandbox = self.parent_sandbox(caller, &workspace).await?;
        if !sandbox.accepts_creation_admission() {
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
        let first_message = request
            .first_message
            .map(|text| FirstMessageRecord::new(creation_key.clone(), text, now.clone()))
            .transpose()
            .map_err(|error| AgentServiceError::InvalidRequest(error.to_string()))?;
        let result = self
            .store
            .admit_agent(AgentAdmission {
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
            })
            .await
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

    pub async fn stop(
        &self,
        caller: &Caller,
        id: &AgentId,
        reason: AgentStopReason,
    ) -> Result<Agent, AgentServiceError> {
        require(caller)?;
        let agent = self.owned_agent(caller, id).await?;
        let scheduled = self
            .store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&agent),
                reason,
                requested_at: now_rfc3339(),
            })
            .await
            .map(|result| result.into_agent())
            .map_err(control_error)?;
        self.runtimes
            .fence(&scheduled.id, scheduled.work.next_fence)
            .await
            .map_err(runtime_error)?;
        Ok(scheduled)
    }

    pub async fn delete(&self, caller: &Caller, id: &AgentId) -> Result<Agent, AgentServiceError> {
        require(caller)?;
        let agent = self.owned_agent(caller, id).await?;
        self.store
            .schedule_agent_delete(AgentDeleteRequest {
                target: AgentControlTarget::from(&agent),
                requested_at: now_rfc3339(),
            })
            .await
            .map(|result| result.into_agent())
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

fn creation_key(origin: &AgentOrigin, agent_id: &AgentId) -> Result<String, AgentServiceError> {
    match origin {
        AgentOrigin::Manual => Ok(format!("manual:{agent_id}")),
        AgentOrigin::Trigger {
            trigger_id,
            launch_id,
        } => {
            if !valid_key_part(trigger_id) || !valid_key_part(launch_id) {
                return Err(AgentServiceError::InvalidRequest(
                    "trigger origin is invalid".into(),
                ));
            }
            content_digest(origin)
                .map(|digest| format!("trigger:{digest}"))
                .map_err(|error| AgentServiceError::InvalidRequest(error.to_string()))
        }
    }
}

fn valid_key_part(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_TRIGGER_KEY_BYTES
        && !value.chars().any(char::is_control)
}

fn validate_title(title: Option<&str>) -> Result<(), AgentServiceError> {
    if title.is_some_and(|value| {
        value.trim().is_empty()
            || value.chars().count() > MAX_TITLE_SCALARS
            || value.chars().any(char::is_control)
    }) {
        return Err(AgentServiceError::InvalidRequest("title is invalid".into()));
    }
    Ok(())
}

fn require(caller: &Caller) -> Result<(), AgentServiceError> {
    caller
        .authorization
        .allows(Capability::UseOwnSandboxes)
        .then_some(())
        .ok_or(AgentServiceError::Forbidden)
}

fn require_launch(caller: &Caller, authority: Capability) -> Result<(), AgentServiceError> {
    (matches!(
        authority,
        Capability::LaunchTriggers | Capability::CreateSandbox
    ) && caller.authorization.allows(authority))
    .then_some(())
    .ok_or(AgentServiceError::Forbidden)
}

fn profile_error(error: ProfileStoreError) -> AgentServiceError {
    match error {
        ProfileStoreError::NotFound => AgentServiceError::ProfileNotFound,
        ProfileStoreError::Corrupt(detail) => AgentServiceError::Corrupt(detail),
        other => AgentServiceError::Unavailable(other.to_string()),
    }
}

fn store_error(error: SandboxStoreError) -> AgentServiceError {
    match error {
        SandboxStoreError::NotFound => AgentServiceError::NotFound,
        SandboxStoreError::WorkspaceNotReady => AgentServiceError::WorkspaceNotReady,
        SandboxStoreError::SandboxNotReady => AgentServiceError::SandboxNotReady,
        SandboxStoreError::AgentLimitReached { limit, live } => {
            AgentServiceError::LimitReached { limit, live }
        }
        SandboxStoreError::AgentCreationConflict => AgentServiceError::CreationConflict,
        SandboxStoreError::AgentLogCapacityExceeded => AgentServiceError::LogCapacityExceeded,
        SandboxStoreError::Corrupt(detail) => AgentServiceError::Corrupt(detail),
        SandboxStoreError::InvalidTransaction(detail) => AgentServiceError::Corrupt(detail.into()),
        other => AgentServiceError::Unavailable(other.to_string()),
    }
}

fn control_error(error: AgentControlStoreError) -> AgentServiceError {
    match error {
        AgentControlStoreError::NotFound => AgentServiceError::NotFound,
        AgentControlStoreError::StatusConflict { status } => {
            AgentServiceError::StatusConflict { status }
        }
        AgentControlStoreError::WorkConflict { step } => AgentServiceError::WorkConflict { step },
        AgentControlStoreError::StopReasonConflict => AgentServiceError::StopReasonConflict,
        AgentControlStoreError::Conflict => AgentServiceError::ControlConflict,
        AgentControlStoreError::Corrupt(detail) => AgentServiceError::Corrupt(detail),
        AgentControlStoreError::Unavailable(detail) => AgentServiceError::Unavailable(detail),
    }
}

fn runtime_error(error: AgentRuntimeError) -> AgentServiceError {
    AgentServiceError::Unavailable(error.to_string())
}
