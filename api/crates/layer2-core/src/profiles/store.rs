use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};

use super::{
    AgentProfile, AgentProfileRevision, MarketplaceLocator, ProfileCapacityBounds, ProfileError,
    ProfileVisibility, SandboxProfile, SandboxProfileRevision, WorkspaceProfile,
    WorkspaceProfileRevision,
};

mod summaries;
pub use summaries::{ProfileSummary, PublishedProfileSummary};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileStoreError {
    #[error("profile not found")]
    NotFound,
    #[error("profile already exists")]
    Duplicate,
    #[error("profile draft changed")]
    Conflict,
    #[error("profile reference is unavailable")]
    ReferenceUnavailable,
    #[error("deployment-owned profile is protected")]
    Protected,
    #[error("profile store unavailable: {0}")]
    Unavailable(String),
    #[error("stored profile data is not usable: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Validation(#[from] ProfileError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilePage<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilePageRequest {
    pub limit: u16,
    pub cursor: Option<String>,
}

#[async_trait]
pub trait ProfileUseRecorder: Send + Sync {
    async fn record_profile_use(
        &self,
        locator: &MarketplaceLocator,
    ) -> Result<(), ProfileStoreError>;
}

impl Default for ProfilePageRequest {
    fn default() -> Self {
        Self {
            limit: 100,
            cursor: None,
        }
    }
}

#[async_trait]
pub trait AgentProfileStore: Send + Sync {
    async fn create_agent_profile(&self, profile: AgentProfile) -> Result<(), ProfileStoreError>;
    async fn agent_profile(
        &self,
        id: &AgentProfileId,
    ) -> Result<Option<AgentProfile>, ProfileStoreError>;
    async fn save_agent_draft(
        &self,
        expected_draft_version: u64,
        profile: AgentProfile,
    ) -> Result<(), ProfileStoreError>;
    async fn publish_agent_profile(
        &self,
        id: &AgentProfileId,
        expected_draft_version: u64,
        published_at: String,
    ) -> Result<AgentProfileRevision, ProfileStoreError>;

    /// Publish only revision one for an exact, unpublished private draft.
    /// Replays must verify the retained aggregate and immutable initial content;
    /// they must never allocate another revision. Implementations without an
    /// atomic initial-publication contract fail closed.
    async fn ensure_initial_agent_profile(
        &self,
        _profile: &AgentProfile,
        _published_at: String,
    ) -> Result<AgentProfileRevision, ProfileStoreError> {
        Err(ProfileStoreError::Unavailable(
            "atomic initial profile publication is unsupported".into(),
        ))
    }

    async fn agent_profile_revision(
        &self,
        id: &AgentProfileId,
        revision: u32,
    ) -> Result<Option<AgentProfileRevision>, ProfileStoreError>;
    async fn list_agent_profile_revisions(
        &self,
        id: &AgentProfileId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<AgentProfileRevision>, ProfileStoreError>;
    async fn list_agent_profiles(
        &self,
        owner: Option<&SubjectId>,
        public_only: bool,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<AgentProfile>, ProfileStoreError>;
    /// List at most 200 owner profiles with their latest verified publications
    /// from one snapshot. Internal profiles are excluded; retirement and private
    /// visibility do not remove the owner's compatibility records.
    async fn list_agent_profile_summaries(
        &self,
        _owner: &SubjectId,
        _page: &ProfilePageRequest,
    ) -> Result<ProfilePage<ProfileSummary<AgentProfile>>, ProfileStoreError> {
        Err(ProfileStoreError::Unavailable(
            "profile summary reads are unsupported".into(),
        ))
    }

    async fn retire_agent_profile(
        &self,
        id: &AgentProfileId,
        owner: &SubjectId,
        retired_at: String,
    ) -> Result<(), ProfileStoreError>;
}

#[async_trait]
pub trait WorkspaceProfileStore: Send + Sync {
    async fn create_workspace_profile(
        &self,
        profile: WorkspaceProfile,
    ) -> Result<(), ProfileStoreError>;
    async fn workspace_profile(
        &self,
        id: &WorkspaceProfileId,
    ) -> Result<Option<WorkspaceProfile>, ProfileStoreError>;
    async fn save_workspace_draft(
        &self,
        expected_draft_version: u64,
        profile: WorkspaceProfile,
    ) -> Result<(), ProfileStoreError>;
    async fn publish_workspace_profile(
        &self,
        id: &WorkspaceProfileId,
        expected_draft_version: u64,
        published_at: String,
    ) -> Result<WorkspaceProfileRevision, ProfileStoreError>;

    /// Publish only revision one for an exact, unpublished private draft.
    /// Replays must verify the retained aggregate and immutable initial content;
    /// they must never allocate another revision. Implementations without an
    /// atomic initial-publication contract fail closed.
    async fn ensure_initial_workspace_profile(
        &self,
        _profile: &WorkspaceProfile,
        _published_at: String,
    ) -> Result<WorkspaceProfileRevision, ProfileStoreError> {
        Err(ProfileStoreError::Unavailable(
            "atomic initial profile publication is unsupported".into(),
        ))
    }

    async fn workspace_profile_revision(
        &self,
        id: &WorkspaceProfileId,
        revision: u32,
    ) -> Result<Option<WorkspaceProfileRevision>, ProfileStoreError>;
    async fn list_workspace_profile_revisions(
        &self,
        id: &WorkspaceProfileId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<WorkspaceProfileRevision>, ProfileStoreError>;
    async fn list_workspace_profiles(
        &self,
        owner: Option<&SubjectId>,
        public_only: bool,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<WorkspaceProfile>, ProfileStoreError>;
    /// List at most 200 owner profiles with their latest verified publications
    /// from one snapshot. Internal profiles are excluded; retirement and private
    /// visibility do not remove the owner's compatibility records.
    async fn list_workspace_profile_summaries(
        &self,
        _owner: &SubjectId,
        _page: &ProfilePageRequest,
    ) -> Result<ProfilePage<ProfileSummary<WorkspaceProfile>>, ProfileStoreError> {
        Err(ProfileStoreError::Unavailable(
            "profile summary reads are unsupported".into(),
        ))
    }

    async fn retire_workspace_profile(
        &self,
        id: &WorkspaceProfileId,
        owner: &SubjectId,
        retired_at: String,
    ) -> Result<(), ProfileStoreError>;
}

#[async_trait]
pub trait SandboxProfileStore: Send + Sync {
    async fn sandbox_profile(
        &self,
        id: &SandboxProfileId,
    ) -> Result<Option<SandboxProfile>, ProfileStoreError>;
    async fn create_sandbox_profile(
        &self,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError>;
    async fn save_sandbox_draft(
        &self,
        expected_draft_version: u64,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError>;
    async fn publish_sandbox_profile(
        &self,
        id: &SandboxProfileId,
        expected_draft_version: u64,
        published_at: String,
        bounds: ProfileCapacityBounds,
        scanner: &dyn super::PublicValueScanner,
    ) -> Result<SandboxProfileRevision, ProfileStoreError>;

    /// Publish only revision one for an exact, unpublished private draft.
    /// Replays must verify the retained aggregate and immutable initial content;
    /// they must never allocate another revision. Implementations without an
    /// atomic initial-publication contract fail closed.
    async fn ensure_initial_sandbox_profile(
        &self,
        _profile: &SandboxProfile,
        _published_at: String,
        _bounds: ProfileCapacityBounds,
        _scanner: &dyn super::PublicValueScanner,
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        Err(ProfileStoreError::Unavailable(
            "atomic initial profile publication is unsupported".into(),
        ))
    }

    async fn sandbox_profile_revision(
        &self,
        id: &SandboxProfileId,
        revision: u32,
    ) -> Result<Option<SandboxProfileRevision>, ProfileStoreError>;
    async fn list_sandbox_profile_revisions(
        &self,
        id: &SandboxProfileId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<SandboxProfileRevision>, ProfileStoreError>;
    async fn list_sandbox_profiles(
        &self,
        owner: Option<&SubjectId>,
        public_only: bool,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<SandboxProfile>, ProfileStoreError>;
    /// List at most 200 owner profiles with their latest verified publications
    /// from one snapshot. Internal profiles are excluded; retirement and private
    /// visibility do not remove the owner's compatibility records.
    async fn list_sandbox_profile_summaries(
        &self,
        _owner: &SubjectId,
        _page: &ProfilePageRequest,
    ) -> Result<ProfilePage<ProfileSummary<SandboxProfile>>, ProfileStoreError> {
        Err(ProfileStoreError::Unavailable(
            "profile summary reads are unsupported".into(),
        ))
    }

    async fn retire_sandbox_profile(
        &self,
        id: &SandboxProfileId,
        owner: &SubjectId,
        retired_at: String,
    ) -> Result<(), ProfileStoreError>;
}

#[derive(Clone, Default)]
pub struct InMemoryProfileStore {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    agents: HashMap<AgentProfileId, AgentProfile>,
    agent_revisions: HashMap<(AgentProfileId, u32), AgentProfileRevision>,
    workspaces: HashMap<WorkspaceProfileId, WorkspaceProfile>,
    workspace_revisions: HashMap<(WorkspaceProfileId, u32), WorkspaceProfileRevision>,
    sandboxes: HashMap<SandboxProfileId, SandboxProfile>,
    sandbox_revisions: HashMap<(SandboxProfileId, u32), SandboxProfileRevision>,
}

impl InMemoryProfileStore {
    pub(crate) fn with_sandbox_availability<T>(
        &self,
        id: &SandboxProfileId,
        apply: impl FnOnce(super::SandboxProfileAvailability) -> T,
    ) -> T {
        let state = self.lock();
        apply(
            state
                .sandboxes
                .get(id)
                .map(|profile| profile.availability)
                .unwrap_or_default(),
        )
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn profile_page<T: Clone>(
    mut values: Vec<(String, T)>,
    request: &ProfilePageRequest,
) -> Result<ProfilePage<T>, ProfileStoreError> {
    if request.limit == 0 || request.limit > 200 {
        return Err(ProfileError::new("invalid_page", "limit").into());
    }
    values.sort_by(|left, right| left.0.cmp(&right.0));
    if let Some(cursor) = &request.cursor {
        values.retain(|(key, _)| key > cursor);
    }
    let has_more = values.len() > usize::from(request.limit);
    values.truncate(usize::from(request.limit));
    let next = has_more
        .then(|| values.last().map(|(key, _)| key.clone()))
        .flatten();
    Ok(ProfilePage {
        items: values.into_iter().map(|(_, value)| value).collect(),
        next,
    })
}

fn revision_page<T: Clone>(
    mut values: Vec<(u32, T)>,
    request: &ProfilePageRequest,
) -> Result<ProfilePage<T>, ProfileStoreError> {
    if request.limit == 0 || request.limit > 200 {
        return Err(ProfileError::new("invalid_page", "limit").into());
    }
    values.sort_by_key(|row| std::cmp::Reverse(row.0));
    if let Some(cursor) = &request.cursor {
        let cursor = cursor
            .parse::<u32>()
            .map_err(|_| ProfileError::new("invalid_page", "cursor"))?;
        values.retain(|(revision, _)| *revision < cursor);
    }
    let has_more = values.len() > usize::from(request.limit);
    values.truncate(usize::from(request.limit));
    let next = has_more
        .then(|| values.last().map(|(revision, _)| revision.to_string()))
        .flatten();
    Ok(ProfilePage {
        items: values.into_iter().map(|(_, value)| value).collect(),
        next,
    })
}

fn next_revision<I: Clone + Eq + std::hash::Hash, R>(
    revisions: &HashMap<(I, u32), R>,
    id: &I,
) -> Result<u32, ProfileStoreError> {
    revisions
        .keys()
        .filter(|(stored, _)| stored == id)
        .map(|(_, revision)| *revision)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(ProfileStoreError::Conflict)
}

mod admin_memory;
mod marketplace_memory;
mod marketplace_public_values;
mod marketplace_support;
mod memory;
mod memory_publication;
mod reference_memory;
