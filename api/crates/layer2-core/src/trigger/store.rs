use async_trait::async_trait;

use super::{
    Launch, LaunchId, LaunchState, PublicTriggerId, TriggerDraft, TriggerId, TriggerRevision,
    TriggerRevisionId, WorkerLease, WorkspaceCheckpoint, WorkspaceManifest,
};
use crate::SubjectId;

pub const DEFAULT_TRIGGER_PAGE_SIZE: u32 = 50;
pub const MAX_TRIGGER_PAGE_SIZE: u32 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerPageRequest {
    pub limit: u32,
    pub cursor: Option<String>,
}

impl TriggerPageRequest {
    pub fn new(limit: u32, cursor: Option<String>) -> Self {
        Self {
            limit: limit.clamp(1, MAX_TRIGGER_PAGE_SIZE),
            cursor,
        }
    }
}

impl Default for TriggerPageRequest {
    fn default() -> Self {
        Self::new(DEFAULT_TRIGGER_PAGE_SIZE, None)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TriggerPage<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriggerStoreError {
    #[error("trigger persistence is unavailable: {0}")]
    Unavailable(String),
    #[error("stored trigger data is unusable: {0}")]
    Malformed(String),
    #[error("resource already exists")]
    AlreadyExists,
    #[error("resource revision changed: expected {expected}, found {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("worker fence changed: expected {expected}, found {actual}")]
    StaleFence { expected: u64, actual: u64 },
    #[error("the resource is held by another live worker")]
    LeaseHeld,
    #[error("this Launch was superseded by an explicit New Launch")]
    Superseded,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PublishedRevision {
    pub draft: TriggerDraft,
    pub revision: TriggerRevision,
}

#[async_trait]
pub trait TriggerStore: Send + Sync {
    async fn insert_draft(&self, draft: &TriggerDraft) -> Result<(), TriggerStoreError>;

    async fn get_draft(&self, id: &TriggerId) -> Result<Option<TriggerDraft>, TriggerStoreError>;

    async fn list_drafts_for(
        &self,
        owner: &SubjectId,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerDraft>, TriggerStoreError>;

    async fn replace_draft(
        &self,
        expected_version: u64,
        draft: &TriggerDraft,
    ) -> Result<TriggerDraft, TriggerStoreError>;

    /// Atomically advances the draft and inserts the immutable revision.
    async fn publish_revision(
        &self,
        expected_draft_version: u64,
        draft: &TriggerDraft,
        revision: &TriggerRevision,
    ) -> Result<PublishedRevision, TriggerStoreError>;

    async fn get_revision(
        &self,
        id: &TriggerRevisionId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError>;

    async fn get_public_revision(
        &self,
        id: &PublicTriggerId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError>;

    async fn list_revisions_for(
        &self,
        owner: &SubjectId,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerRevision>, TriggerStoreError>;

    async fn retire_revision(
        &self,
        id: &TriggerRevisionId,
        owner: &SubjectId,
        retired_at: &str,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchQuery {
    pub owner: Option<SubjectId>,
    pub states: Vec<LaunchState>,
    pub updated_before: Option<String>,
    pub page: TriggerPageRequest,
}

#[async_trait]
pub trait LaunchStore: Send + Sync {
    /// Inserts a draft, or returns the current Launch for the same owner and revision.
    async fn insert_or_get_current(&self, launch: &Launch) -> Result<Launch, TriggerStoreError>;

    async fn insert_new(&self, launch: &Launch) -> Result<(), TriggerStoreError>;

    async fn get_launch(&self, id: &LaunchId) -> Result<Option<Launch>, TriggerStoreError>;

    async fn list_launches(
        &self,
        query: &LaunchQuery,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError>;

    /// Replaces one aggregate only when both its optimistic revision and worker fence match.
    async fn replace_launch(
        &self,
        expected_revision: u64,
        expected_fence: Option<u64>,
        launch: &Launch,
    ) -> Result<Launch, TriggerStoreError>;

    /// Atomically installs a monotonically increasing worker lease when no live lease exists.
    async fn acquire_lease(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        holder: &str,
        acquired_at: &str,
        expires_at: &str,
    ) -> Result<Option<Launch>, TriggerStoreError>;

    async fn release_lease(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        lease: &WorkerLease,
    ) -> Result<Launch, TriggerStoreError>;
}

#[async_trait]
pub trait CheckpointObjectStore: Send + Sync {
    async fn put_checkpoint(
        &self,
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
        archive: &[u8],
    ) -> Result<(), TriggerStoreError>;

    async fn get_checkpoint(
        &self,
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError>;

    async fn delete_checkpoint(
        &self,
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
    ) -> Result<(), TriggerStoreError>;
}

/// Durable byte storage for the immutable repository baseline.
///
/// Every operation carries the authenticated owner and Launch identity even
/// though the underlying bucket is reached by one CMA machine identity. An
/// implementation must derive and verify its object key from both values; an
/// opaque reference alone never authorizes a cross-subject read.
#[async_trait]
pub trait BaselineObjectStore: Send + Sync {
    async fn put_baseline(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        manifest: &WorkspaceManifest,
        archive_sha256: &str,
        archive: &[u8],
    ) -> Result<String, TriggerStoreError>;

    async fn get_baseline(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        reference: &str,
        archive_sha256: &str,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError>;

    async fn delete_baseline(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        reference: &str,
    ) -> Result<(), TriggerStoreError>;
}
