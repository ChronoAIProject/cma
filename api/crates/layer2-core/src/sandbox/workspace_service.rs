use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::profiles::store::{ProfileStoreError, ProfileUseRecorder, WorkspaceProfileStore};
use crate::profiles::{
    require_trust_confirmation, MarketplaceError, MarketplaceLocator, ProfileVisibility,
    WorkspaceLocation, WorkspaceProfileRef, WorkspaceProfileRevision, WorkspaceProfileSnapshot,
};
use crate::{now_rfc3339, Caller, Capability, SandboxId, SubjectId, WorkspaceId};

use super::service_trust::{validate_workspace_profile, validate_workspace_revision};
use super::store::{
    Page, PageRequest, SandboxStoreError, WorkspaceAdmission, WorkspaceAdmissionKind,
    WorkspaceAdmissionResult, WorkspaceServiceStore,
};
use super::{
    Sandbox, Workspace, WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState,
    WorkspaceWorkStep,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspace {
    pub profile: WorkspaceProfileRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceServiceError {
    #[error("forbidden")]
    Forbidden,
    #[error("workspace profile not found")]
    ProfileNotFound,
    #[error("workspace not found")]
    NotFound,
    #[error("sandbox is not active")]
    SandboxNotReady,
    #[error("workspace name is already in use")]
    NameConflict,
    #[error("workspace limit reached: {live} of {limit}")]
    LimitReached { limit: u8, live: u8 },
    #[error("workspace has live agents")]
    HasLiveAgents,
    #[error("trust confirmation required for digest {digest}")]
    TrustConfirmationRequired { digest: String },
    #[error("workspace profile is invalid: {0}")]
    InvalidProfile(String),
    #[error("workspace provisioning origin conflicts with an existing workspace")]
    OriginConflict,
    #[error("workspace launch origin is invalid")]
    InvalidOrigin,
    #[error("workspace data is not usable: {0}")]
    Corrupt(String),
    #[error("workspace dependency is unavailable: {0}")]
    Unavailable(String),
}

impl WorkspaceServiceError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::ProfileNotFound => "workspace_profile_not_found",
            Self::NotFound => "workspace_not_found",
            Self::SandboxNotReady => "sandbox_not_ready",
            Self::NameConflict => "workspace_name_conflict",
            Self::LimitReached { .. } => "workspace_limit_reached",
            Self::HasLiveAgents => "workspace_has_live_agents",
            Self::TrustConfirmationRequired { .. } => "trust_confirmation_required",
            Self::InvalidProfile(_) => "workspace_profile_invalid",
            Self::OriginConflict => "workspace_origin_conflict",
            Self::InvalidOrigin => "invalid_workspace_origin",
            Self::Corrupt(_) => "stored_workspace_corrupt",
            Self::Unavailable(_) => "workspace_service_unavailable",
        }
    }
}

pub struct WorkspaceService {
    store: Arc<dyn WorkspaceServiceStore>,
    profiles: Arc<dyn WorkspaceProfileStore>,
    profile_uses: Arc<dyn ProfileUseRecorder>,
}

impl WorkspaceService {
    pub fn new<S, P>(store: Arc<S>, profiles: Arc<P>) -> Arc<Self>
    where
        S: WorkspaceServiceStore + 'static,
        P: WorkspaceProfileStore + ProfileUseRecorder + 'static,
    {
        let workspace_profiles: Arc<dyn WorkspaceProfileStore> = profiles.clone();
        let profile_uses: Arc<dyn ProfileUseRecorder> = profiles;
        Self::with_dependencies(store, workspace_profiles, profile_uses)
    }

    pub(crate) fn with_dependencies<S>(
        store: Arc<S>,
        profiles: Arc<dyn WorkspaceProfileStore>,
        profile_uses: Arc<dyn ProfileUseRecorder>,
    ) -> Arc<Self>
    where
        S: WorkspaceServiceStore + 'static,
    {
        Arc::new(Self {
            store,
            profiles,
            profile_uses,
        })
    }

    pub async fn create(
        &self,
        caller: &Caller,
        sandbox_id: &SandboxId,
        request: CreateWorkspace,
    ) -> Result<Workspace, WorkspaceServiceError> {
        require(caller)?;
        let sandbox = self.owned_sandbox(caller, sandbox_id).await?;
        if sandbox.status != super::SandboxStatus::Active {
            return Err(WorkspaceServiceError::SandboxNotReady);
        }
        let (owner, revision) = self
            .read_visible_revision(&caller.subject, &request.profile)
            .await?;
        require_trust_confirmation(
            &owner,
            &caller.subject,
            !revision.content.setup_commands.is_empty(),
            &revision.digest,
            request.confirmed_digest.as_deref(),
        )
        .map_err(trust_error)?;
        self.admit(
            &sandbox,
            &revision,
            WorkspaceOrigin::Manual,
            WorkspaceAdmissionKind::Manual,
        )
        .await
    }

    /// Launch-authorized admission with a durable origin used to reconcile
    /// retries after an uncertain storage response.
    pub async fn create_for_launch(
        &self,
        caller: &Caller,
        sandbox_id: &SandboxId,
        request: CreateWorkspace,
        launch_id: String,
    ) -> Result<Workspace, WorkspaceServiceError> {
        if launch_id.trim().is_empty()
            || launch_id.len() > 128
            || launch_id.chars().any(char::is_control)
        {
            return Err(WorkspaceServiceError::InvalidOrigin);
        }
        require(caller)?;
        let sandbox = self.owned_sandbox(caller, sandbox_id).await?;
        if sandbox.status != super::SandboxStatus::Active {
            return Err(WorkspaceServiceError::SandboxNotReady);
        }
        let (owner, revision) = self
            .read_visible_revision(&caller.subject, &request.profile)
            .await?;
        require_trust_confirmation(
            &owner,
            &caller.subject,
            !revision.content.setup_commands.is_empty(),
            &revision.digest,
            request.confirmed_digest.as_deref(),
        )
        .map_err(trust_error)?;
        self.admit(
            &sandbox,
            &revision,
            WorkspaceOrigin::Trigger { launch_id },
            WorkspaceAdmissionKind::Trigger,
        )
        .await
    }

    /// Parent-authorized provisioning path used while the sandbox is Installing.
    pub async fn create_profile_workspace(
        &self,
        sandbox: &Sandbox,
        profile: &WorkspaceProfileRef,
        index: u16,
    ) -> Result<Workspace, WorkspaceServiceError> {
        if sandbox
            .profile
            .content
            .workspace_profiles
            .get(usize::from(index))
            != Some(profile)
        {
            return Err(WorkspaceServiceError::OriginConflict);
        }
        let revision = self.exact_revision(profile).await?;
        self.admit(
            sandbox,
            &revision,
            WorkspaceOrigin::SandboxProfile { index },
            WorkspaceAdmissionKind::SandboxProfile,
        )
        .await
    }

    pub async fn list(
        &self,
        caller: &Caller,
        sandbox_id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<Workspace>, WorkspaceServiceError> {
        require(caller)?;
        self.owned_sandbox(caller, sandbox_id).await?;
        self.store
            .list_workspaces(sandbox_id, page)
            .await
            .map_err(store_error)
    }

    pub async fn read(
        &self,
        caller: &Caller,
        id: &WorkspaceId,
    ) -> Result<Workspace, WorkspaceServiceError> {
        require(caller)?;
        self.owned_workspace(caller, id).await
    }

    pub async fn remove(
        &self,
        caller: &Caller,
        id: &WorkspaceId,
    ) -> Result<Workspace, WorkspaceServiceError> {
        require(caller)?;
        self.owned_workspace(caller, id).await?;
        self.store
            .begin_workspace_removal(id, &caller.subject, now_rfc3339())
            .await
            .map_err(store_error)
    }

    async fn admit(
        &self,
        sandbox: &Sandbox,
        revision: &WorkspaceProfileRevision,
        origin: WorkspaceOrigin,
        kind: WorkspaceAdmissionKind,
    ) -> Result<Workspace, WorkspaceServiceError> {
        let now = now_rfc3339();
        let workspace = workspace(sandbox, revision, origin, now.clone())?;
        let result = self
            .store
            .admit_workspace(WorkspaceAdmission {
                workspace,
                kind,
                event_at: now,
            })
            .await
            .map_err(store_error)?;
        match result {
            WorkspaceAdmissionResult::Existing(workspace) => Ok(workspace),
            WorkspaceAdmissionResult::Created(workspace) => {
                let locator = MarketplaceLocator::Workspace(workspace.profile.id.clone());
                if let Err(error) = self.profile_uses.record_profile_use(&locator).await {
                    tracing::warn!(
                        workspace_id = %workspace.id,
                        profile_id = locator.as_str(),
                        %error,
                        "workspace profile use was not recorded"
                    );
                }
                Ok(workspace)
            }
        }
    }

    async fn read_visible_revision(
        &self,
        consumer: &SubjectId,
        reference: &WorkspaceProfileRef,
    ) -> Result<(SubjectId, WorkspaceProfileRevision), WorkspaceServiceError> {
        let profile = self
            .profiles
            .workspace_profile(&reference.id)
            .await
            .map_err(profile_error)?
            .ok_or(WorkspaceServiceError::ProfileNotFound)?;
        validate_workspace_profile(reference, &profile).map_err(profile_error)?;
        if profile.retired_at.is_some()
            || (&profile.owner != consumer && profile.visibility != ProfileVisibility::Public)
        {
            return Err(WorkspaceServiceError::ProfileNotFound);
        }
        let revision = self.exact_revision(reference).await?;
        Ok((profile.owner, revision))
    }

    async fn exact_revision(
        &self,
        reference: &WorkspaceProfileRef,
    ) -> Result<WorkspaceProfileRevision, WorkspaceServiceError> {
        let revision = self
            .profiles
            .workspace_profile_revision(&reference.id, reference.revision)
            .await
            .map_err(profile_error)?
            .ok_or(WorkspaceServiceError::ProfileNotFound)?;
        validate_workspace_revision(reference, &revision).map_err(profile_error)?;
        Ok(revision)
    }

    async fn owned_sandbox(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<Sandbox, WorkspaceServiceError> {
        self.store
            .get_sandbox(id)
            .await
            .map_err(store_error)?
            .filter(|sandbox| sandbox.owner == caller.subject)
            .ok_or(WorkspaceServiceError::NotFound)
    }

    async fn owned_workspace(
        &self,
        caller: &Caller,
        id: &WorkspaceId,
    ) -> Result<Workspace, WorkspaceServiceError> {
        self.store
            .get_workspace(id)
            .await
            .map_err(store_error)?
            .filter(|workspace| workspace.owner == caller.subject)
            .ok_or(WorkspaceServiceError::NotFound)
    }
}

fn workspace(
    sandbox: &Sandbox,
    revision: &WorkspaceProfileRevision,
    origin: WorkspaceOrigin,
    now: String,
) -> Result<Workspace, WorkspaceServiceError> {
    revision
        .content
        .validate()
        .map_err(|error| WorkspaceServiceError::InvalidProfile(error.to_string()))?;
    let name = revision
        .content
        .workspace_name()
        .map_err(|error| WorkspaceServiceError::InvalidProfile(error.to_string()))?;
    let source = match &revision.content.location {
        WorkspaceLocation::GitRepository { url, reference } => WorkspaceSource::GitRepository {
            url: url.clone(),
            reference: reference.clone(),
        },
        WorkspaceLocation::Directory { .. } => WorkspaceSource::Empty,
    };
    Ok(Workspace {
        id: WorkspaceId::generate(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin,
        profile: WorkspaceProfileSnapshot::from(revision),
        name: name.clone(),
        path: format!("/home/cma/{name}"),
        source,
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, now.clone()),
        next_worktree_number: 1,
        created_at: now,
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    })
}

fn require(caller: &Caller) -> Result<(), WorkspaceServiceError> {
    caller
        .authorization
        .allows(Capability::UseOwnSandboxes)
        .then_some(())
        .ok_or(WorkspaceServiceError::Forbidden)
}

fn trust_error(error: MarketplaceError) -> WorkspaceServiceError {
    match error {
        MarketplaceError::TrustConfirmationRequired { commands_digest } => {
            WorkspaceServiceError::TrustConfirmationRequired {
                digest: commands_digest,
            }
        }
        other => WorkspaceServiceError::Unavailable(other.to_string()),
    }
}

fn profile_error(error: ProfileStoreError) -> WorkspaceServiceError {
    match error {
        ProfileStoreError::NotFound => WorkspaceServiceError::ProfileNotFound,
        ProfileStoreError::Corrupt(detail) => WorkspaceServiceError::Corrupt(detail),
        other => WorkspaceServiceError::Unavailable(other.to_string()),
    }
}

fn store_error(error: SandboxStoreError) -> WorkspaceServiceError {
    match error {
        SandboxStoreError::NotFound => WorkspaceServiceError::NotFound,
        SandboxStoreError::SandboxNotReady => WorkspaceServiceError::SandboxNotReady,
        SandboxStoreError::WorkspaceNameConflict | SandboxStoreError::Duplicate => {
            WorkspaceServiceError::NameConflict
        }
        SandboxStoreError::WorkspaceLimitReached { limit, live } => {
            WorkspaceServiceError::LimitReached { limit, live }
        }
        SandboxStoreError::WorkspaceHasLiveAgents => WorkspaceServiceError::HasLiveAgents,
        SandboxStoreError::WorkspaceOriginConflict => WorkspaceServiceError::OriginConflict,
        SandboxStoreError::Corrupt(detail) => WorkspaceServiceError::Corrupt(detail),
        other => WorkspaceServiceError::Unavailable(other.to_string()),
    }
}
