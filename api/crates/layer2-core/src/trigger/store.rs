use async_trait::async_trait;

use super::{
    BaselineArchivePlan, BaselineImportBlob, BaselineImportChunk, BaselineImportOutcome,
    CapabilityRevocationState, DiffArtifact, DiffArtifactId, Launch, LaunchEffectKind, LaunchEvent,
    LaunchId, LaunchState, PublicTriggerId, TriggerConnection, TriggerDraft, TriggerId,
    TriggerRevision, TriggerRevisionId, WorkerLease, WorkspaceCheckpoint,
    WorkspaceCheckpointIntent, WorkspaceManifest,
};
use crate::{SessionId, SubjectId};

pub const DEFAULT_TRIGGER_PAGE_SIZE: u32 = 50;
pub const MAX_TRIGGER_PAGE_SIZE: u32 = 200;

pub const TRIGGER_WORKER_STATES: [LaunchState; 12] = [
    LaunchState::Preflight,
    LaunchState::ForkPending,
    LaunchState::ForkReady,
    LaunchState::BranchPending,
    LaunchState::WorkspacePending,
    LaunchState::SandboxPending,
    LaunchState::SessionPending,
    LaunchState::PromptPending,
    LaunchState::Contributing,
    LaunchState::PrPending,
    LaunchState::PrOpen,
    LaunchState::FailedRetryable,
];

pub const TRIGGER_CAPABILITY_STATES: [LaunchState; 10] = [
    LaunchState::PromptPending,
    LaunchState::Ready,
    LaunchState::Contributing,
    LaunchState::PrPending,
    LaunchState::PrOpen,
    LaunchState::Completed,
    LaunchState::AuthRequired,
    LaunchState::FailedRetryable,
    LaunchState::FailedTerminal,
    LaunchState::Cancelled,
];

pub const TRIGGER_WORKSPACE_STATES: [LaunchState; 15] = [
    LaunchState::Draft,
    LaunchState::AuthRequired,
    LaunchState::Preflight,
    LaunchState::ForkPending,
    LaunchState::ForkReady,
    LaunchState::BranchPending,
    LaunchState::WorkspacePending,
    LaunchState::SandboxPending,
    LaunchState::SessionPending,
    LaunchState::PromptPending,
    LaunchState::Ready,
    LaunchState::Contributing,
    LaunchState::PrPending,
    LaunchState::PrOpen,
    LaunchState::FailedRetryable,
];

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

/// Durable owner proof and provider-observed settlement for one connection revoke.
///
/// The active snapshot is persisted before the provider write. That makes a
/// lost DELETE response recoverable without treating disappearance from an
/// active-only provider listing as either ownership or success evidence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectionRevocationRecord {
    pub owner: SubjectId,
    pub service_id: String,
    pub revision: u64,
    pub active_snapshot: TriggerConnection,
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_receipt: Option<TriggerConnection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_lease: Option<ConnectionRevocationLease>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectionRevocationLease {
    pub holder: String,
    pub acquired_at: String,
    pub expires_at: String,
}

/// Persistence and aggregate fencing used by a reusable connection revoke.
#[async_trait]
pub trait ConnectionRevocationStore: Send + Sync {
    /// Insert the pre-write owner proof, or return the exact existing intent.
    async fn insert_or_get_connection_revocation(
        &self,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, TriggerStoreError>;

    async fn get_connection_revocation(
        &self,
        owner: &SubjectId,
        service_id: &str,
    ) -> Result<Option<ConnectionRevocationRecord>, TriggerStoreError>;

    /// Replace a settled epoch after the provider proves that the same exact
    /// service identity was reactivated at a newer state version.
    async fn restart_connection_revocation(
        &self,
        expected_revision: u64,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, TriggerStoreError>;

    /// Claim the single provider writer. An unexpired holder returns `None`.
    async fn claim_connection_revocation(
        &self,
        owner: &SubjectId,
        service_id: &str,
        lease: &ConnectionRevocationLease,
    ) -> Result<Option<ConnectionRevocationRecord>, TriggerStoreError>;

    /// Persist positive provider evidence while retaining the writer lease.
    async fn record_connection_revocation_receipt(
        &self,
        expected_revision: u64,
        lease_holder: &str,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, TriggerStoreError>;

    /// Fence every affected Launch and preserve all of its retained work.
    async fn fence_launches_for_connection_revocation(
        &self,
        owner: &SubjectId,
        service_id: &str,
        now: &str,
        correlation_id: &str,
    ) -> Result<(), TriggerStoreError>;

    /// Mark both provider settlement and Launch fencing complete.
    async fn settle_connection_revocation(
        &self,
        expected_revision: u64,
        lease_holder: &str,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, TriggerStoreError>;

    /// Release an inconclusive writer so a retry can reconcile immediately.
    async fn release_connection_revocation(
        &self,
        expected_revision: u64,
        lease_holder: &str,
        owner: &SubjectId,
        service_id: &str,
    ) -> Result<(), TriggerStoreError>;

    async fn list_settled_connection_revocations(
        &self,
        owner: &SubjectId,
    ) -> Result<Vec<ConnectionRevocationRecord>, TriggerStoreError>;
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
    pub public_id: Option<PublicTriggerId>,
    pub session_id: Option<SessionId>,
    pub states: Vec<LaunchState>,
    pub updated_before: Option<String>,
    pub page: TriggerPageRequest,
}

#[async_trait]
pub trait LaunchStore: Send + Sync {
    /// Inserts a draft, or returns the current Launch for the same owner and revision.
    async fn insert_or_get_current(&self, launch: &Launch) -> Result<Launch, TriggerStoreError>;

    /// Returns the current README-click Launch without creating a replacement.
    async fn get_current_launch(
        &self,
        owner: &SubjectId,
        revision: &TriggerRevisionId,
    ) -> Result<Option<Launch>, TriggerStoreError>;

    async fn insert_new(&self, launch: &Launch) -> Result<(), TriggerStoreError>;

    /// Atomically settles one still-current expired draft and installs its replacement.
    ///
    /// A losing concurrent README re-click returns the winner's current Launch,
    /// so expiration does not weaken the one-current-Launch invariant.
    async fn expire_current_and_insert(
        &self,
        expired: &Launch,
        replacement: &Launch,
    ) -> Result<Launch, TriggerStoreError>;

    async fn get_launch(&self, id: &LaunchId) -> Result<Option<Launch>, TriggerStoreError>;

    /// Reads the canonical append-only Launch event stream. Implementations
    /// with only an aggregate store may use the retained tail; durable stores
    /// override this with their independent event collection.
    async fn list_launch_events(
        &self,
        id: &LaunchId,
        after: u64,
        limit: usize,
    ) -> Result<Vec<LaunchEvent>, TriggerStoreError> {
        Ok(self
            .get_launch(id)
            .await?
            .map(|launch| {
                launch
                    .events
                    .into_iter()
                    .filter(|event| event.sequence > after)
                    .take(limit.max(1))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Resolve an operator-facing correlation without exposing a collection
    /// scan through the public service contract.
    async fn find_launch_by_correlation_id(
        &self,
        _correlation_id: &str,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        Ok(None)
    }

    async fn list_launches(
        &self,
        query: &LaunchQuery,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError>;

    /// Launches that may claim a normal orchestration effect, including a
    /// predecessor superseded only as the current README-click target.
    async fn list_worker_candidates(
        &self,
        updated_before: Option<String>,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        let mut candidates = self
            .list_launches(&LaunchQuery {
                owner: None,
                public_id: None,
                session_id: None,
                states: TRIGGER_WORKER_STATES.to_vec(),
                updated_before: updated_before.clone(),
                page: page.clone(),
            })
            .await?;
        if let Some(boundary) = updated_before.as_deref() {
            candidates.items.retain(|launch| {
                launch.state != LaunchState::PrOpen
                    || launch.pull_request_status_due(boundary).unwrap_or(true)
            });
        }
        Ok(candidates)
    }

    /// Launches with Reach work or terminal session-key retirement, including
    /// predecessors superseded only as the current README-click target.
    async fn list_capability_candidates(
        &self,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        let mut candidates = self
            .list_launches(&LaunchQuery {
                owner: None,
                public_id: None,
                session_id: None,
                states: TRIGGER_CAPABILITY_STATES.to_vec(),
                updated_before: None,
                page: page.clone(),
            })
            .await?;
        candidates.items.retain(|launch| {
            launch.cancelled_session_requires_settlement()
                || launch.session_credential_requires_retirement()
                || launch
                    .reach
                    .as_ref()
                    .is_some_and(|reach| reach.state != CapabilityRevocationState::Revoked)
        });
        Ok(candidates)
    }

    /// Terminal Launches whose CMA-owned objects crossed their retention fence.
    async fn list_cleanup_candidates(
        &self,
        delete_before: &str,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        let candidates = self
            .list_launches(&LaunchQuery {
                owner: None,
                public_id: None,
                session_id: None,
                states: vec![
                    LaunchState::Completed,
                    LaunchState::FailedTerminal,
                    LaunchState::Cancelled,
                ],
                updated_before: None,
                page: page.clone(),
            })
            .await?;
        Ok(TriggerPage {
            items: candidates
                .items
                .into_iter()
                .filter(|launch| {
                    let completed_baseline_due = launch
                        .workspace
                        .as_ref()
                        .is_some_and(|workspace| workspace.delete_after.as_str() <= delete_before);
                    let staged_import_due = launch.workspace.is_none()
                        && launch
                            .terminal_at
                            .as_deref()
                            .is_some_and(|terminal_at| terminal_at <= delete_before)
                        && launch
                            .effects
                            .iter()
                            .any(|effect| effect.kind == LaunchEffectKind::WorkspaceImport);
                    (completed_baseline_due || staged_import_due)
                        && launch
                            .cleanup
                            .as_ref()
                            .and_then(|cleanup| cleanup.completed_at.as_ref())
                            .is_none()
                })
                .collect(),
            next: candidates.next,
        })
    }

    /// Nonterminal Launches whose exact live Trigger session may be due for a
    /// lifecycle workspace checkpoint. Reservation performs the final cadence
    /// and exclusion check under the Launch CAS.
    async fn list_workspace_candidates(
        &self,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        let candidates = self
            .list_launches(&LaunchQuery {
                owner: None,
                public_id: None,
                session_id: None,
                states: TRIGGER_WORKSPACE_STATES.to_vec(),
                updated_before: None,
                page: page.clone(),
            })
            .await?;
        Ok(TriggerPage {
            items: candidates
                .items
                .into_iter()
                .filter(|launch| launch.session.is_some() && launch.workspace.is_some())
                .collect(),
            next: candidates.next,
        })
    }

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
    ) -> Result<String, TriggerStoreError>;

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

    /// Delete an immutable object whose upload may have completed before its
    /// Launch commit became observable. The exact key is derived from the
    /// still-durable reservation, never from caller-provided path syntax.
    async fn delete_checkpoint_intent(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        intent: &WorkspaceCheckpointIntent,
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
    /// Return a previously finalized exact import without consulting GitHub.
    async fn reconcile_baseline_import(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        source_revision: &str,
    ) -> Result<Option<BaselineImportOutcome>, TriggerStoreError>;

    /// Persist the complete bounded provider-object inventory before chunks.
    async fn begin_baseline_import(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        source_revision: &str,
        tree_digest: &str,
        blobs: &[BaselineImportBlob],
    ) -> Result<(), TriggerStoreError>;

    async fn get_baseline_import_chunk(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        source_revision: &str,
        blob: &BaselineImportBlob,
        ordinal: u32,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError>;

    /// Store one immutable fixed-size chunk. An existing different value must
    /// fail closed rather than being overwritten by a stale worker.
    async fn put_baseline_import_chunk(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        source_revision: &str,
        blob: &BaselineImportBlob,
        chunk: &BaselineImportChunk,
        bytes: &[u8],
    ) -> Result<(), TriggerStoreError>;

    /// Stream the canonical preamble and ordered chunks into the unchanged
    /// whole-baseline object contract, then retain a completed checkpoint.
    async fn finalize_baseline_import(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        manifest: &WorkspaceManifest,
        plan: BaselineArchivePlan,
    ) -> Result<BaselineImportOutcome, TriggerStoreError>;

    /// Delete staged chunks after the completed checkpoint is durable. A
    /// partial failure remains discoverable and retryable from the index.
    async fn retire_baseline_import_chunks(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
    ) -> Result<(), TriggerStoreError>;

    /// Remove all deterministic staging/checkpoint objects, including imports
    /// that never reached a final WorkspaceRecord.
    async fn delete_baseline_import(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
    ) -> Result<(), TriggerStoreError>;

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

/// Tenant-isolated storage for the complete, immutable review artifact.
///
/// The Launch aggregate retains only a bounded summary. This port keeps the
/// downloadable artifact out of MongoDB's document-size boundary and requires
/// every read/delete to re-establish subject, Launch, artifact, and digest.
#[async_trait]
pub trait DiffArtifactObjectStore: Send + Sync {
    async fn put_diff_artifact(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        artifact: &DiffArtifactId,
        sha256: &str,
        bytes: &[u8],
    ) -> Result<String, TriggerStoreError>;

    async fn get_diff_artifact(
        &self,
        owner: &SubjectId,
        artifact: &DiffArtifact,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError>;

    async fn delete_diff_artifact(
        &self,
        owner: &SubjectId,
        artifact: &DiffArtifact,
    ) -> Result<(), TriggerStoreError>;
}
