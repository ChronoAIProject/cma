use async_trait::async_trait;

use crate::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};

use super::{
    Placement, PlacementId, PlacementUpdate, PublicTriggerId, Trigger, TriggerAvailability,
    TriggerError, TriggerId, TriggerPreview, TriggerReferenceError, TriggerRevision, TriggerStats,
};

mod memory;
mod memory_guards;

pub use memory::InMemoryTriggerStore;

pub const DEFAULT_TRIGGER_PAGE_SIZE: u16 = 50;
pub const MAX_TRIGGER_PAGE_SIZE: u16 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerPageRequest {
    pub limit: u16,
    pub cursor: Option<String>,
}

impl Default for TriggerPageRequest {
    fn default() -> Self {
        Self {
            limit: DEFAULT_TRIGGER_PAGE_SIZE,
            cursor: None,
        }
    }
}

impl TriggerPageRequest {
    pub fn validate(&self) -> Result<(), TriggerStoreError> {
        if self.limit == 0 || self.limit > MAX_TRIGGER_PAGE_SIZE {
            Err(TriggerError::new("invalid_trigger_page", "limit").into())
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerPage<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicTrigger {
    pub trigger_id: TriggerId,
    pub public_id: PublicTriggerId,
    pub owner: SubjectId,
    pub availability: TriggerAvailability,
    pub revision: TriggerRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriggerStoreError {
    #[error("trigger not found")]
    NotFound,
    #[error("trigger already exists")]
    Duplicate,
    #[error("trigger draft changed: expected {expected}, found {actual}")]
    Conflict { expected: u64, actual: u64 },
    #[error("trigger has already been published")]
    AlreadyPublished,
    #[error(transparent)]
    Reference(#[from] TriggerReferenceError),
    #[error("profile is referenced by published triggers: {trigger_ids:?}")]
    ProfileReferenced { trigger_ids: Vec<TriggerId> },
    #[error("trigger store unavailable: {0}")]
    Unavailable(String),
    #[error("stored trigger data is not usable: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Validation(#[from] TriggerError),
}

impl TriggerStoreError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "trigger_not_found",
            Self::Duplicate => "trigger_already_exists",
            Self::Conflict { .. } => "trigger_draft_changed",
            Self::AlreadyPublished => "trigger_already_published",
            Self::Reference(error) => error.code(),
            Self::ProfileReferenced { .. } => "profile_referenced_by_published_trigger",
            Self::Unavailable(_) => "trigger_store_unavailable",
            Self::Corrupt(_) => "stored_trigger_corrupt",
            Self::Validation(error) => error.code,
        }
    }
}

#[async_trait]
pub trait TriggerStore: Send + Sync {
    async fn create_trigger(&self, trigger: Trigger) -> Result<(), TriggerStoreError>;

    async fn trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
    ) -> Result<Option<Trigger>, TriggerStoreError>;

    /// Deletes unpublished drafts; archives published definitions without removing Launch authority.
    async fn delete_trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        archived_at: String,
    ) -> Result<(), TriggerStoreError>;

    /// Reads the first immutable revision whose content digest matches exactly.
    ///
    /// Implementations must bound the result to one deterministic row and validate that the
    /// returned revision belongs to `id` and that its content produces `digest`.
    async fn trigger_revision(
        &self,
        id: &TriggerId,
        digest: &str,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError>;

    async fn save_draft(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        expected_draft_version: u64,
        draft: super::TriggerContent,
        updated_at: String,
    ) -> Result<Trigger, TriggerStoreError>;

    /// Publishes one immutable revision after authorizing every exact profile reference.
    ///
    /// Durable implementations must read current profile visibility/retirement state and
    /// commit the Trigger revision atomically against one consistent snapshot. In particular,
    /// a profile retirement or visibility change must not land between authorization and the
    /// revision write. The draft CAS check belongs in that same write boundary. If a profile
    /// mutation wins the race, publication must observe it and fail; if publication wins, a
    /// retirement guard must observe the published reference. A later visibility change does
    /// not invalidate the immutable revision.
    async fn publish_trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        expected_draft_version: u64,
        published_at: String,
    ) -> Result<PublicTrigger, TriggerStoreError>;

    async fn trigger_by_public_id(
        &self,
        id: &PublicTriggerId,
    ) -> Result<Option<PublicTrigger>, TriggerStoreError>;

    async fn preview_by_public_id(
        &self,
        id: &PublicTriggerId,
        publisher_display_name: String,
    ) -> Result<Option<TriggerPreview>, TriggerStoreError>;

    async fn list_own_triggers(
        &self,
        owner: &SubjectId,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Trigger>, TriggerStoreError>;

    async fn retire_trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        retired_at: String,
    ) -> Result<Trigger, TriggerStoreError>;

    async fn create_placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        source_url: String,
        label: String,
    ) -> Result<Placement, TriggerStoreError>;

    async fn placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
    ) -> Result<Option<Placement>, TriggerStoreError>;

    async fn list_placements(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        include_retired: bool,
    ) -> Result<Vec<Placement>, TriggerStoreError>;

    async fn update_placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
        update: PlacementUpdate,
    ) -> Result<Placement, TriggerStoreError>;

    async fn retire_placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
        retired_at: String,
    ) -> Result<Placement, TriggerStoreError>;

    async fn record_launch(&self, id: &TriggerId) -> Result<TriggerStats, TriggerStoreError>;

    async fn record_agent_ready(&self, id: &TriggerId) -> Result<TriggerStats, TriggerStoreError>;

    async fn guard_sandbox_profile_retirement(
        &self,
        id: &SandboxProfileId,
    ) -> Result<(), TriggerStoreError>;

    async fn guard_workspace_profile_retirement(
        &self,
        id: &WorkspaceProfileId,
    ) -> Result<(), TriggerStoreError>;

    async fn guard_agent_profile_retirement(
        &self,
        id: &AgentProfileId,
    ) -> Result<(), TriggerStoreError>;
}
