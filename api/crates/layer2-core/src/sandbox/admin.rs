use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::profiles::{
    ProfileAdminQuery, ProfileAdminStore, ProfileAdminSummary, ProfileLocator, ProfilePage,
    ProfileStoreError,
};
use crate::{
    now_rfc3339, ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget,
    Caller, Capability, DetailKey, SandboxId, SubjectId, WorkspaceId, WorktreeId,
};

use super::reviewed_retirement::ReviewedRetirementStore;
use super::store::{
    Page, PageRequest, SandboxStore, SandboxStoreError, SandboxTransaction, SandboxTransactionStore,
};
use super::{
    Agent, AgentStatus, Sandbox, SandboxEvent, SandboxEventKind, SandboxStatus, Workspace,
    WorkspaceStatus,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandboxAdminQuery {
    pub owner: Option<SubjectId>,
    pub status: Option<SandboxStatus>,
    pub runtime_version: Option<String>,
    /// True selects explicit exceptions; false includes unset preferences.
    pub always_on: Option<bool>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxAdminSummary {
    pub id: SandboxId,
    pub owner: SubjectId,
    pub name: String,
    pub status: SandboxStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    pub agent_count: u64,
    pub last_activity_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl SandboxAdminSummary {
    pub fn from_sandbox(sandbox: &Sandbox, agent_count: u64) -> Self {
        Self {
            id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            name: sandbox.name.clone(),
            status: sandbox.status,
            runtime_version: sandbox.runtime.as_ref().map(|r| r.runtime_version.clone()),
            agent_count,
            last_activity_at: sandbox.last_activity_at.clone(),
            retired_at: sandbox.retired_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAdminSummary {
    pub id: WorkspaceId,
    pub name: String,
    pub status: WorkspaceStatus,
    pub profile_id: String,
    pub profile_revision: u32,
}

impl From<&Workspace> for WorkspaceAdminSummary {
    fn from(workspace: &Workspace) -> Self {
        Self {
            id: workspace.id.clone(),
            name: workspace.name.clone(),
            status: workspace.status,
            profile_id: workspace.profile.id.to_string(),
            profile_revision: workspace.profile.revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAdminSummary {
    pub id: crate::AgentId,
    pub workspace_id: WorkspaceId,
    pub worktree_id: WorktreeId,
    pub status: AgentStatus,
    pub profile_id: String,
    pub profile_revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub updated_at: String,
}

impl From<&Agent> for AgentAdminSummary {
    fn from(agent: &Agent) -> Self {
        Self {
            id: agent.id.clone(),
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
            status: agent.status,
            profile_id: agent.profile.id.to_string(),
            profile_revision: agent.profile.revision,
            title: agent.title.clone(),
            updated_at: agent.updated_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxAdminView {
    pub sandbox: Sandbox,
    pub workspaces: Vec<WorkspaceAdminSummary>,
    pub agents: Vec<AgentAdminSummary>,
}

#[async_trait]
pub trait SandboxAdminStore: Send + Sync {
    async fn list_admin_sandboxes(
        &self,
        query: &SandboxAdminQuery,
    ) -> Result<Page<SandboxAdminSummary>, SandboxStoreError>;

    async fn read_admin_sandbox(
        &self,
        id: &SandboxId,
    ) -> Result<Option<SandboxAdminView>, SandboxStoreError>;

    async fn read_admin_sandbox_events(
        &self,
        id: &SandboxId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<SandboxEvent>, SandboxStoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRetirementResult {
    pub sandbox: Sandbox,
    pub changed: bool,
}

#[async_trait]
pub trait SandboxRetirement: Send + Sync {
    async fn retire(
        &self,
        id: &SandboxId,
        requested_at: String,
        expected_revision: Option<u64>,
    ) -> Result<SandboxRetirementResult, OperatorError>;
}

pub trait SandboxRetirementStore: SandboxStore + SandboxTransactionStore {}

impl<T> SandboxRetirementStore for T where T: SandboxStore + SandboxTransactionStore {}

pub struct StoredSandboxRetirement {
    store: Arc<dyn SandboxRetirementStore>,
}

impl StoredSandboxRetirement {
    pub fn new<S>(store: Arc<S>) -> Self
    where
        S: SandboxRetirementStore + 'static,
    {
        Self { store }
    }
}

#[async_trait]
impl SandboxRetirement for StoredSandboxRetirement {
    async fn retire(
        &self,
        id: &SandboxId,
        requested_at: String,
        expected_revision: Option<u64>,
    ) -> Result<SandboxRetirementResult, OperatorError> {
        let mut sandbox = self
            .store
            .get_sandbox(id)
            .await
            .map_err(OperatorError::from_sandbox_store)?
            .ok_or(OperatorError::NotFound)?;
        if expected_revision.is_some_and(|revision| revision != sandbox.revision) {
            return Err(OperatorError::InvalidState(
                "sandbox retirement revision changed".into(),
            ));
        }
        let resume = expected_revision.is_some() && sandbox.retirement_can_resume();
        if !resume
            && matches!(
                sandbox.status,
                SandboxStatus::Retiring | SandboxStatus::Retired
            )
        {
            return Ok(SandboxRetirementResult {
                sandbox,
                changed: false,
            });
        }
        let expected_revision = sandbox.revision;
        let previous = sandbox.status;
        if resume {
            sandbox.resume_retirement(requested_at.clone())
        } else {
            sandbox.begin_retirement(requested_at.clone())
        }
        .map_err(|error| OperatorError::InvalidState(error.to_string()))?;
        self.store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision,
                sandbox: Box::new(sandbox.clone()),
                event_at: requested_at,
                event: SandboxEventKind::StatusChanged {
                    from: previous,
                    to: SandboxStatus::Retiring,
                },
            })
            .await
            .map_err(OperatorError::from_sandbox_store)?;
        Ok(SandboxRetirementResult {
            sandbox,
            changed: true,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OperatorError {
    #[error("forbidden")]
    Forbidden,
    #[error("operator target not found")]
    NotFound,
    #[error("invalid operator request: {0}")]
    InvalidRequest(String),
    #[error("operator action conflicts with current state: {0}")]
    InvalidState(String),
    #[error("deployment-owned profile is protected")]
    ProtectedProfile,
    #[error("operator dependency unavailable: {0}")]
    Unavailable(String),
}

mod always_on;
mod errors;
mod reviewed_retirement;

pub struct SandboxOperatorService {
    sandboxes: Arc<dyn SandboxAdminStore>,
    profiles: Arc<dyn ProfileAdminStore>,
    retirement: Arc<dyn SandboxRetirement>,
    reviewed_retirement: Option<ReviewedRetirementAccess>,
    maintenance: Option<Arc<dyn super::SandboxMaintenanceStore>>,
    activity: Arc<ActivityService>,
}

struct ReviewedRetirementAccess {
    store: Arc<dyn ReviewedRetirementStore>,
    admit: bool,
}

impl SandboxOperatorService {
    pub fn new(
        sandboxes: Arc<dyn SandboxAdminStore>,
        profiles: Arc<dyn ProfileAdminStore>,
        retirement: Arc<dyn SandboxRetirement>,
        activity: Arc<ActivityService>,
    ) -> Self {
        Self {
            sandboxes,
            profiles,
            retirement,
            reviewed_retirement: None,
            maintenance: None,
            activity,
        }
    }

    pub async fn list_sandboxes(
        &self,
        caller: &Caller,
        query: &SandboxAdminQuery,
    ) -> Result<Page<SandboxAdminSummary>, OperatorError> {
        require(caller, Capability::ReadAllSandboxes)?;
        let page = self
            .sandboxes
            .list_admin_sandboxes(query)
            .await
            .map_err(OperatorError::from_sandbox_store)?;
        let mut record = activity(caller, ActivityAction::List, "sandboxes");
        record
            .detail
            .record(DetailKey::Limit, query.page.limit.to_string());
        record_optional_filters(
            &mut record,
            query.owner.as_ref(),
            query.status.map(SandboxStatus::name),
            query.runtime_version.as_deref(),
        );
        self.append(record).await?;
        Ok(page)
    }

    pub async fn read_sandbox(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<SandboxAdminView, OperatorError> {
        require(caller, Capability::ReadAllSandboxes)?;
        let view = self
            .sandboxes
            .read_admin_sandbox(id)
            .await
            .map_err(OperatorError::from_sandbox_store)?
            .ok_or(OperatorError::NotFound)?;
        let mut record = activity(caller, ActivityAction::Read, &sandbox_target(id));
        record
            .detail
            .record(DetailKey::Subject, view.sandbox.owner.as_str());
        record
            .detail
            .record(DetailKey::Status, view.sandbox.status.name());
        self.append(record).await?;
        Ok(view)
    }

    pub async fn sandbox_events(
        &self,
        caller: &Caller,
        id: &SandboxId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<SandboxEvent>, OperatorError> {
        require(caller, Capability::ReadAllSandboxes)?;
        let events = self
            .sandboxes
            .read_admin_sandbox_events(id, after, limit)
            .await
            .map_err(OperatorError::from_sandbox_store)?;
        let target = format!("sandbox:{id}:events");
        let mut record = activity(caller, ActivityAction::Read, &target);
        record.detail.record(DetailKey::Cursor, after.to_string());
        record.detail.record(DetailKey::Limit, limit.to_string());
        self.append(record).await?;
        Ok(events)
    }

    pub async fn list_profiles(
        &self,
        caller: &Caller,
        query: &ProfileAdminQuery,
    ) -> Result<ProfilePage<ProfileAdminSummary>, OperatorError> {
        require(caller, Capability::ReadAllSandboxes)?;
        let page = self
            .profiles
            .list_profiles(query)
            .await
            .map_err(OperatorError::from_profile_store)?;
        let mut record = activity(caller, ActivityAction::List, "profiles");
        record
            .detail
            .record(DetailKey::Limit, query.page.limit.to_string());
        if let Some(owner) = &query.owner {
            record.detail.record(DetailKey::Subject, owner.as_str());
        }
        if let Some(kind) = query.kind {
            record.detail.record(DetailKey::ProfileKind, kind.as_str());
        }
        if let Some(visibility) = query.visibility {
            record
                .detail
                .record(DetailKey::Visibility, visibility.as_str());
        }
        self.append(record).await?;
        Ok(page)
    }

    pub async fn retire_sandbox(
        &self,
        caller: &Caller,
        id: &SandboxId,
        expected_revision: Option<u64>,
    ) -> Result<SandboxRetirementResult, OperatorError> {
        require(caller, Capability::ControlAnySandbox)?;
        let result = self
            .retirement
            .retire(id, now_rfc3339(), expected_revision)
            .await?;
        let mut record = activity(caller, ActivityAction::Delete, &sandbox_target(id));
        record
            .detail
            .record(DetailKey::Subject, result.sandbox.owner.as_str());
        record
            .detail
            .record(DetailKey::Status, result.sandbox.status.name());
        record
            .detail
            .record(DetailKey::Revision, result.sandbox.revision.to_string());
        self.append(record).await?;
        Ok(result)
    }

    pub async fn retire_profile(
        &self,
        caller: &Caller,
        locator: &ProfileLocator,
    ) -> Result<ProfileAdminSummary, OperatorError> {
        require(caller, Capability::ControlAnySandbox)?;
        if deployment_profile(locator) {
            return Err(OperatorError::ProtectedProfile);
        }
        let summary = self
            .profiles
            .retire_profile(locator, now_rfc3339())
            .await
            .map_err(OperatorError::from_profile_store)?;
        let target = format!("profile:{}:{}", locator.kind().as_str(), locator.as_str());
        let mut record = activity(caller, ActivityAction::Delete, &target);
        record
            .detail
            .record(DetailKey::Subject, summary.owner.as_str());
        record
            .detail
            .record(DetailKey::ProfileKind, summary.kind.as_str());
        record
            .detail
            .record(DetailKey::Visibility, summary.visibility.as_str());
        self.append(record).await?;
        Ok(summary)
    }

    async fn append(&self, record: ActivityRecord) -> Result<(), OperatorError> {
        self.activity
            .record(&record)
            .await
            .map_err(|error| OperatorError::Unavailable(error.to_string()))
    }
}

fn deployment_profile(locator: &ProfileLocator) -> bool {
    match locator {
        ProfileLocator::Agent(id) => id.as_str() == crate::DEFAULT_AGENT_PROFILE_ID,
        ProfileLocator::Workspace(id) => id.as_str() == crate::DEFAULT_WORKSPACE_PROFILE_ID,
        ProfileLocator::Sandbox(id) => id.as_str() == crate::DEFAULT_SANDBOX_PROFILE_ID,
    }
}

fn require(caller: &Caller, capability: Capability) -> Result<(), OperatorError> {
    caller
        .require(capability)
        .map_err(|_| OperatorError::Forbidden)
}

fn activity(caller: &Caller, action: ActivityAction, target: &str) -> ActivityRecord {
    ActivityRecord::new(action, ActivityOutcome::Succeeded)
        .by(caller.subject.clone())
        .on(ActivityTarget::Resource(target.into()))
}

fn record_optional_filters(
    record: &mut ActivityRecord,
    owner: Option<&SubjectId>,
    status: Option<&str>,
    runtime_version: Option<&str>,
) {
    if let Some(owner) = owner {
        record.detail.record(DetailKey::Subject, owner.as_str());
    }
    if let Some(status) = status {
        record.detail.record(DetailKey::Status, status);
    }
    if let Some(runtime_version) = runtime_version {
        record.detail.record(DetailKey::Runtime, runtime_version);
    }
}

fn sandbox_target(id: &SandboxId) -> String {
    format!("sandbox:{id}")
}

#[cfg(test)]
mod tests;
