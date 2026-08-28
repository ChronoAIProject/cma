use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization as _;

use super::{
    CheckpointId, CommitIdentity, DiffArtifactId, LaunchId, ScopeClass, TriggerValidationError,
    WorkflowChangePolicy,
};
use crate::SessionId;

pub const WORKSPACE_MANIFEST_VERSION: u32 = 1;
pub const MAX_REPOSITORY_FILES: usize = 100_000;
pub const MAX_REPOSITORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Fixed storage checkpoint unit for restartable baseline imports.
pub const BASELINE_IMPORT_CHUNK_BYTES: u64 = 4 * 1024 * 1024;
/// GitHub's JSON blob route materializes one base64 value. Refuse a single
/// provider object above this bound before allocating or requesting it.
pub const MAX_PROVIDER_BLOB_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_PATH_BYTES: usize = 1_024;
/// Absolute schema safety ceiling. The lower deployment-owned publication
/// limit is captured in every artifact after measurement against the pinned
/// broker/provider path.
pub const MAX_DIFF_FILES: usize = 10_000;
pub const MAX_DIFF_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_DIFF_SUMMARY_FILES: usize = 1_000;
pub const MAX_DIFF_SUMMARY_PATCH_BYTES: usize = 256 * 1024;
pub const MAX_DIFF_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
/// Maximum time a healthy Trigger workspace may remain without another
/// coherent capture attempt. Unchanged captures advance the deadline without
/// writing another object.
pub const LIFECYCLE_CHECKPOINT_INTERVAL_SECONDS: u64 = 5 * 60;
pub const LIFECYCLE_CHECKPOINT_RETRY_SECONDS: u64 = 30;
const EMPTY_CONTENT_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceEntryKind {
    Regular,
    Symlink,
    Submodule,
}

/// Immutable provider object inventory persisted before any blob is fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineImportBlob {
    pub provider_object_id: String,
    pub bytes: u64,
    pub chunk_count: u32,
}

impl BaselineImportBlob {
    pub fn new(
        provider_object_id: impl Into<String>,
        bytes: u64,
    ) -> Result<Self, TriggerValidationError> {
        let provider_object_id = provider_object_id.into().to_ascii_lowercase();
        if provider_object_id.len() != 40
            || !provider_object_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(TriggerValidationError::field(
                "baseline_import.provider_object_id",
                "must be a Git SHA-1",
            ));
        }
        if bytes > MAX_PROVIDER_BLOB_BYTES {
            return Err(TriggerValidationError::field(
                "baseline_import.bytes",
                format!("exceeds the {MAX_PROVIDER_BLOB_BYTES}-byte provider blob limit"),
            ));
        }
        let chunk_count = if bytes == 0 {
            0
        } else {
            u32::try_from(bytes.div_ceil(BASELINE_IMPORT_CHUNK_BYTES)).map_err(|_| {
                TriggerValidationError::field(
                    "baseline_import.chunk_count",
                    "exceeds the durable import bound",
                )
            })?
        };
        Ok(Self {
            provider_object_id,
            bytes,
            chunk_count,
        })
    }
}

/// One verified immutable checkpoint used to assemble the canonical archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineImportChunk {
    pub provider_object_id: String,
    pub ordinal: u32,
    pub bytes: u64,
    pub sha256: String,
}

/// Ordered, bounded recipe for the final whole-baseline object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineArchivePlan {
    pub source_revision: String,
    pub tree_digest: String,
    pub preamble: Vec<u8>,
    pub chunks: Vec<BaselineImportChunk>,
    pub archive_bytes: u64,
}

/// Durable evidence that the canonical baseline object is complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineImportOutcome {
    pub manifest: WorkspaceManifest,
    pub object_reference: String,
    pub archive_sha256: String,
    pub archive_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub path: String,
    pub kind: WorkspaceEntryKind,
    pub mode: u32,
    pub size: u64,
    pub content_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_object_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
}

impl WorkspaceEntry {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        validate_workspace_path(&self.path)?;
        validate_sha256("workspace.entries.content_sha256", &self.content_sha256)?;
        validate_optional_object_id(
            "workspace.entries.provider_object_id",
            self.provider_object_id.as_deref(),
        )?;
        match self.kind {
            WorkspaceEntryKind::Regular => {
                if !matches!(self.mode, 0o100644 | 0o100755) || self.symlink_target.is_some() {
                    return Err(TriggerValidationError::field(
                        format!("workspace.entries.{}", self.path),
                        "regular files must use a supported regular mode and no symlink target",
                    ));
                }
            }
            WorkspaceEntryKind::Symlink => {
                if self.mode != 0o120000 {
                    return Err(TriggerValidationError::field(
                        format!("workspace.entries.{}", self.path),
                        "a symlink must use mode 120000",
                    ));
                }
                validate_symlink_target(
                    &self.path,
                    self.symlink_target.as_deref().unwrap_or_default(),
                )?;
            }
            WorkspaceEntryKind::Submodule => {
                if self.mode != 0o160000
                    || self.size != 0
                    || self.content_sha256 != EMPTY_CONTENT_SHA256
                    || self.provider_object_id.is_none()
                    || self.symlink_target.is_some()
                {
                    return Err(TriggerValidationError::field(
                        format!("workspace.entries.{}", self.path),
                        "a submodule must be an empty mode-160000 entry pinned to a Git commit",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    pub version: u32,
    pub source_revision: String,
    pub entries: Vec<WorkspaceEntry>,
    pub total_bytes: u64,
    pub digest: String,
}

impl WorkspaceManifest {
    pub fn seal(
        source_revision: impl Into<String>,
        mut entries: Vec<WorkspaceEntry>,
    ) -> Result<Self, TriggerValidationError> {
        if entries.len() > MAX_REPOSITORY_FILES {
            return Err(TriggerValidationError::field(
                "workspace.entries",
                format!("at most {MAX_REPOSITORY_FILES} files are supported"),
            ));
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let mut seen_aliases = BTreeSet::new();
        let mut total_bytes = 0_u64;
        for entry in &entries {
            entry.validate()?;
            let alias = workspace_path_alias(&entry.path)?;
            if !seen_aliases.insert(alias) {
                return Err(TriggerValidationError::field(
                    "workspace.entries",
                    format!("{} aliases another workspace path", entry.path),
                ));
            }
            total_bytes = total_bytes.checked_add(entry.size).ok_or_else(|| {
                TriggerValidationError::field("workspace.total_bytes", "overflowed")
            })?;
        }
        if total_bytes > MAX_REPOSITORY_BYTES {
            return Err(TriggerValidationError::field(
                "workspace.total_bytes",
                format!("exceeds the {MAX_REPOSITORY_BYTES}-byte repository limit"),
            ));
        }
        if seen_aliases.iter().any(|alias| {
            let mut parent = alias.as_str();
            while let Some((prefix, _)) = parent.rsplit_once('/') {
                if seen_aliases.contains(prefix) {
                    return true;
                }
                parent = prefix;
            }
            false
        }) {
            return Err(TriggerValidationError::field(
                "workspace.entries",
                "a workspace path aliases a file as a directory",
            ));
        }
        let source_revision = source_revision.into();
        if source_revision.trim().is_empty() || source_revision.len() > 128 {
            return Err(TriggerValidationError::field(
                "workspace.source_revision",
                "is not usable",
            ));
        }
        let digest = manifest_digest(&source_revision, &entries)?;
        Ok(Self {
            version: WORKSPACE_MANIFEST_VERSION,
            source_revision,
            entries,
            total_bytes,
            digest,
        })
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.version != WORKSPACE_MANIFEST_VERSION {
            return Err(TriggerValidationError::field(
                "workspace.version",
                "is not supported",
            ));
        }
        let sealed = Self::seal(self.source_revision.clone(), self.entries.clone())?;
        if sealed.total_bytes != self.total_bytes || sealed.digest != self.digest {
            return Err(TriggerValidationError::field(
                "workspace.digest",
                "does not match the canonical manifest",
            ));
        }
        Ok(())
    }
}

fn manifest_digest(
    source_revision: &str,
    entries: &[WorkspaceEntry],
) -> Result<String, TriggerValidationError> {
    let mut digest = Sha256::new();
    digest.update(b"cma-workspace-manifest-v1\0");
    hash_text(&mut digest, source_revision)?;
    digest.update(
        u64::try_from(entries.len())
            .map_err(|_| TriggerValidationError::new("workspace manifest is too large"))?
            .to_be_bytes(),
    );
    for entry in entries {
        hash_text(&mut digest, &entry.path)?;
        digest.update([match entry.kind {
            WorkspaceEntryKind::Regular => 1,
            WorkspaceEntryKind::Symlink => 2,
            WorkspaceEntryKind::Submodule => 3,
        }]);
        digest.update(entry.mode.to_be_bytes());
        digest.update(entry.size.to_be_bytes());
        hash_text(&mut digest, &entry.content_sha256)?;
        hash_optional_text(&mut digest, entry.provider_object_id.as_deref())?;
        hash_optional_text(&mut digest, entry.symlink_target.as_deref())?;
    }
    Ok(hex(&digest.finalize()))
}

fn hash_text(digest: &mut Sha256, value: &str) -> Result<(), TriggerValidationError> {
    digest.update(
        u64::try_from(value.len())
            .map_err(|_| TriggerValidationError::new("workspace manifest value is too large"))?
            .to_be_bytes(),
    );
    digest.update(value.as_bytes());
    Ok(())
}

fn hash_optional_text(
    digest: &mut Sha256,
    value: Option<&str>,
) -> Result<(), TriggerValidationError> {
    match value {
        Some(value) => {
            digest.update([1]);
            hash_text(digest, value)
        }
        None => {
            digest.update([0]);
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointIntegrity {
    Pending,
    Complete,
    Corrupt,
    RecoveryRequired,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceCheckpointPurpose {
    Periodic,
    PreTeardown,
    ContributionReview,
}

impl WorkspaceCheckpointPurpose {
    pub fn is_lifecycle(self) -> bool {
        matches!(self, Self::Periodic | Self::PreTeardown)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleCheckpointState {
    #[default]
    Idle,
    Capturing,
    RetryScheduled,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCheckpointFailure {
    pub code: String,
    pub detail: String,
    pub observed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<String>,
}

/// One durable version reservation. A process may disappear after capture or
/// upload and another process will still address the same immutable object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCheckpointIntent {
    pub id: CheckpointId,
    pub version: u64,
    pub purpose: WorkspaceCheckpointPurpose,
    pub session_id: SessionId,
    pub runtime_generation: u64,
    /// Exact Launch workspace authority this reservation may mutate.
    #[serde(default)]
    pub workspace_epoch: u64,
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleCheckpointStatus {
    pub state: LifecycleCheckpointState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<WorkspaceCheckpointFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceCheckpointReservation {
    NotDue,
    Busy,
    Capture(WorkspaceCheckpointIntent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCheckpointCommitOutcome {
    Unchanged,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCheckpoint {
    pub id: CheckpointId,
    pub launch_id: LaunchId,
    pub version: u64,
    pub workspace_epoch: u64,
    pub session_id: SessionId,
    pub runtime_generation: u64,
    pub object_reference: String,
    pub archive_sha256: String,
    pub manifest: WorkspaceManifest,
    pub integrity: CheckpointIntegrity,
    /// Present only when this workspace was committed with the exact opaque
    /// conversation object needed to recover the same runtime boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_pair: Option<crate::TriggerRecoveryPairRef>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_after: Option<String>,
}

impl WorkspaceCheckpoint {
    pub fn validate_complete(&self, launch: &LaunchId) -> Result<(), TriggerValidationError> {
        if &self.launch_id != launch
            || self.workspace_epoch == 0
            || self.version == 0
            || self.session_id.0.trim().is_empty()
            || self.runtime_generation == 0
        {
            return Err(TriggerValidationError::field(
                "checkpoint",
                "does not belong to the requested Launch workspace and runtime generation",
            ));
        }
        if self.integrity != CheckpointIntegrity::Complete {
            return Err(TriggerValidationError::field(
                "checkpoint.integrity",
                "is not complete",
            ));
        }
        if self.object_reference.trim().is_empty() {
            return Err(TriggerValidationError::field(
                "checkpoint.object_reference",
                "is absent",
            ));
        }
        validate_sha256("checkpoint.archive_sha256", &self.archive_sha256)?;
        self.manifest.validate()?;
        if let Some(pair) = self.recovery_pair.as_ref() {
            pair.validate_checkpoint(self)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffChangeKind {
    Added,
    Modified,
    Deleted,
    ModeChanged,
    SymlinkChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFile {
    pub path: String,
    pub change: DiffChangeKind,
    pub binary: bool,
    pub old_mode: Option<u32>,
    pub new_mode: Option<u32>,
    pub old_sha256: Option<String>,
    pub new_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_provider_object_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_provider_object_id: Option<String>,
    pub old_bytes: u64,
    pub new_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// False for binary/oversized content and for a Launch summary whose full
    /// patch is retained only in the downloadable artifact.
    pub patch_complete: bool,
}

impl DiffFile {
    fn validate(&self) -> Result<(), TriggerValidationError> {
        validate_workspace_path(&self.path)?;
        validate_optional_sha256("diff.files.old_sha256", self.old_sha256.as_deref())?;
        validate_optional_sha256("diff.files.new_sha256", self.new_sha256.as_deref())?;
        validate_optional_object_id(
            "diff.files.old_provider_object_id",
            self.old_provider_object_id.as_deref(),
        )?;
        validate_optional_object_id(
            "diff.files.new_provider_object_id",
            self.new_provider_object_id.as_deref(),
        )?;
        validate_optional_diff_mode("diff.files.old_mode", self.old_mode)?;
        validate_optional_diff_mode("diff.files.new_mode", self.new_mode)?;
        if self.binary && self.patch.is_some() {
            return Err(TriggerValidationError::field(
                "diff.files.patch",
                "binary content cannot carry a rendered text patch",
            ));
        }
        if self.patch.is_some() && !self.patch_complete {
            return Err(TriggerValidationError::field(
                "diff.files.patch_complete",
                "a retained patch must be complete",
            ));
        }

        let old_present = self.old_mode.is_some() && self.old_sha256.is_some();
        let new_present = self.new_mode.is_some() && self.new_sha256.is_some();
        let shape_is_valid = match self.change {
            DiffChangeKind::Added => {
                !old_present
                    && self.old_mode.is_none()
                    && self.old_sha256.is_none()
                    && self.old_provider_object_id.is_none()
                    && self.old_bytes == 0
                    && new_present
            }
            DiffChangeKind::Deleted => {
                old_present
                    && !new_present
                    && self.new_mode.is_none()
                    && self.new_sha256.is_none()
                    && self.new_provider_object_id.is_none()
                    && self.new_bytes == 0
            }
            DiffChangeKind::Modified => {
                old_present
                    && new_present
                    && (self.old_sha256 != self.new_sha256 || self.old_mode != self.new_mode)
            }
            DiffChangeKind::ModeChanged => {
                old_present
                    && new_present
                    && self.old_sha256 == self.new_sha256
                    && self.old_mode != self.new_mode
                    && self.old_bytes == self.new_bytes
            }
            DiffChangeKind::SymlinkChanged => {
                old_present
                    && new_present
                    && (self.old_mode == Some(0o120000) || self.new_mode == Some(0o120000))
                    && (self.old_sha256 != self.new_sha256 || self.old_mode != self.new_mode)
            }
        };
        if !shape_is_valid {
            return Err(TriggerValidationError::field(
                "diff.files",
                "change metadata is internally inconsistent",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffValidationOutcome {
    Valid,
    NoChanges,
    StaleHead,
    WorkflowScopeRequired,
    TooManyChanges,
    TooManyBytes,
    UnsupportedFile,
    MissingBaseline,
    CorruptCheckpoint,
    ArtifactTooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub key: String,
    pub passed: bool,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffArtifact {
    pub id: DiffArtifactId,
    pub launch_id: LaunchId,
    pub checkpoint_id: CheckpointId,
    pub baseline_digest: String,
    pub checkpoint_digest: String,
    pub expected_branch_head: String,
    pub observed_branch_head: String,
    pub changed_file_limit: u32,
    pub total_changed_files: u32,
    pub files: Vec<DiffFile>,
    pub changed_bytes: u64,
    pub digest: String,
    pub object_reference: String,
    pub object_sha256: String,
    pub object_bytes: u64,
    pub author: CommitIdentity,
    pub committer: CommitIdentity,
    #[serde(default)]
    pub workflow_paths: Vec<String>,
    pub validation: DiffValidationOutcome,
    #[serde(default)]
    pub checks: Vec<CheckResult>,
    pub created_at: String,
    pub delete_after: String,
}

impl DiffArtifact {
    /// Validate a retained review outcome without treating it as publication
    /// authority. Invalid outcomes remain useful to the contributor, but they
    /// can never satisfy `validate_for_publication`.
    pub fn validate_for_review(
        &self,
        launch: &super::Launch,
    ) -> Result<(), TriggerValidationError> {
        if self.launch_id != launch.id {
            return Err(TriggerValidationError::field(
                "diff.launch_id",
                "does not belong to this Launch",
            ));
        }
        let configured_limit = usize::try_from(self.changed_file_limit).unwrap_or(usize::MAX);
        let total_changed_files = usize::try_from(self.total_changed_files).unwrap_or(usize::MAX);
        if self.changed_file_limit == 0
            || configured_limit > MAX_DIFF_FILES
            || self.files.len() > MAX_DIFF_SUMMARY_FILES
            || self.files.len() > total_changed_files
        {
            return Err(TriggerValidationError::field(
                "diff.files",
                "is outside the bounded review-artifact contract",
            ));
        }
        if self.changed_bytes > MAX_REPOSITORY_BYTES.saturating_mul(2) {
            return Err(TriggerValidationError::field(
                "diff.changed_bytes",
                "is outside the imported workspace bound",
            ));
        }
        match self.validation {
            DiffValidationOutcome::Valid => {
                if total_changed_files == 0
                    || total_changed_files > configured_limit
                    || self.changed_bytes > MAX_DIFF_BYTES
                    || self.files.is_empty()
                {
                    return Err(TriggerValidationError::field(
                        "diff.validation",
                        "valid requires one complete change set within publication limits",
                    ));
                }
            }
            DiffValidationOutcome::NoChanges => {
                if total_changed_files != 0 || !self.files.is_empty() || self.changed_bytes != 0 {
                    return Err(TriggerValidationError::field(
                        "diff.validation",
                        "no_changes cannot retain changed files or bytes",
                    ));
                }
            }
            DiffValidationOutcome::TooManyChanges => {
                if total_changed_files <= configured_limit {
                    return Err(TriggerValidationError::field(
                        "diff.validation",
                        "too_many_changes requires the measured limit to be exceeded",
                    ));
                }
            }
            DiffValidationOutcome::TooManyBytes => {
                if self.changed_bytes <= MAX_DIFF_BYTES {
                    return Err(TriggerValidationError::field(
                        "diff.validation",
                        "too_many_bytes requires the byte limit to be exceeded",
                    ));
                }
            }
            DiffValidationOutcome::StaleHead => {
                if self.observed_branch_head == self.expected_branch_head {
                    return Err(TriggerValidationError::field(
                        "diff.validation",
                        "stale_head requires a differing observed branch head",
                    ));
                }
            }
            DiffValidationOutcome::WorkflowScopeRequired => {
                if self.workflow_paths.is_empty() {
                    return Err(TriggerValidationError::field(
                        "diff.validation",
                        "workflow_scope_required must identify a workflow path",
                    ));
                }
            }
            DiffValidationOutcome::UnsupportedFile
            | DiffValidationOutcome::MissingBaseline
            | DiffValidationOutcome::CorruptCheckpoint
            | DiffValidationOutcome::ArtifactTooLarge => {
                if total_changed_files != 0 || !self.files.is_empty() || self.changed_bytes != 0 {
                    return Err(TriggerValidationError::field(
                        "diff.validation",
                        "this validation outcome must use a compact metadata-only artifact",
                    ));
                }
            }
        }

        validate_sha256("diff.baseline_digest", &self.baseline_digest)?;
        validate_sha256("diff.checkpoint_digest", &self.checkpoint_digest)?;
        validate_sha256("diff.digest", &self.digest)?;
        validate_sha256("diff.object_sha256", &self.object_sha256)?;
        if self.object_reference.trim().is_empty()
            || self.object_bytes == 0
            || self.object_bytes > MAX_DIFF_ARTIFACT_BYTES
            || self.object_sha256 != self.digest
        {
            return Err(TriggerValidationError::field(
                "diff.object_reference",
                "does not identify the exact bounded review artifact",
            ));
        }
        self.author.validate()?;
        self.committer.validate()?;

        let workspace = launch.workspace.as_ref().ok_or_else(|| {
            TriggerValidationError::field("diff.baseline_digest", "has no retained baseline")
        })?;
        let checkpoint = workspace
            .sealed_contribution_checkpoint
            .as_ref()
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "diff.checkpoint_id",
                    "has no sealed contribution checkpoint",
                )
            })?;
        let branch = launch.branch.as_ref().ok_or_else(|| {
            TriggerValidationError::field("diff.expected_branch_head", "has no Launch branch")
        })?;
        if self.checkpoint_id != checkpoint.id
            || self.baseline_digest != workspace.baseline.digest
            || self.checkpoint_digest != checkpoint.manifest.digest
            || self.expected_branch_head != branch.current_head
            || self.delete_after != workspace.delete_after
        {
            return Err(TriggerValidationError::field(
                "diff",
                "does not match the retained baseline, checkpoint, branch, and retention fence",
            ));
        }

        let mut paths = BTreeSet::new();
        let mut summary_patch_bytes = 0_usize;
        for file in &self.files {
            file.validate()?;
            if !paths.insert(file.path.as_str()) {
                return Err(TriggerValidationError::field(
                    "diff.files",
                    "contains a duplicate path",
                ));
            }
            summary_patch_bytes = summary_patch_bytes
                .saturating_add(file.patch.as_ref().map_or(0, |patch| patch.len()));
        }
        if summary_patch_bytes > MAX_DIFF_SUMMARY_PATCH_BYTES {
            return Err(TriggerValidationError::field(
                "diff.files.patch",
                "exceeds the Launch summary patch bound",
            ));
        }

        let mut prior_workflow_path: Option<&str> = None;
        for path in &self.workflow_paths {
            validate_workspace_path(path)?;
            if !path.starts_with(".github/workflows/")
                || prior_workflow_path.is_some_and(|prior| prior >= path.as_str())
            {
                return Err(TriggerValidationError::field(
                    "diff.workflow_paths",
                    "must be a sorted unique set of workflow paths",
                ));
            }
            prior_workflow_path = Some(path);
        }
        let summarized_workflow_paths = self
            .files
            .iter()
            .filter(|file| file.path.starts_with(".github/workflows/"))
            .map(|file| file.path.as_str())
            .collect::<BTreeSet<_>>();
        if summarized_workflow_paths.iter().any(|path| {
            !self
                .workflow_paths
                .iter()
                .any(|candidate| candidate == path)
        }) || (self.files.len() == total_changed_files
            && summarized_workflow_paths.len() != self.workflow_paths.len())
        {
            return Err(TriggerValidationError::field(
                "diff.workflow_paths",
                "does not match the reviewed path set",
            ));
        }
        if self.checks.len() > 32
            || self.checks.iter().any(|check| {
                check.key.trim().is_empty() || check.key.len() > 64 || check.summary.len() > 2_000
            })
            || self
                .checks
                .iter()
                .map(|check| check.key.as_str())
                .collect::<BTreeSet<_>>()
                .len()
                != self.checks.len()
        {
            return Err(TriggerValidationError::field(
                "diff.checks",
                "is outside the bounded check-result contract",
            ));
        }
        Ok(())
    }

    pub fn validate_for_publication(
        &self,
        launch: &super::Launch,
    ) -> Result<(), TriggerValidationError> {
        self.validate_for_review(launch)?;
        if self.validation != DiffValidationOutcome::Valid {
            return Err(TriggerValidationError::field(
                "diff",
                "is not a valid reviewed artifact for this Launch",
            ));
        }
        let configured_limit = usize::try_from(self.changed_file_limit).unwrap_or(usize::MAX);
        if self.files.is_empty()
            || self.files.len() > configured_limit
            || usize::try_from(self.total_changed_files).ok() != Some(self.files.len())
        {
            return Err(TriggerValidationError::field(
                "diff.files",
                "is outside the measured publication limit",
            ));
        }
        if self.changed_bytes > MAX_DIFF_BYTES {
            return Err(TriggerValidationError::field(
                "diff.changed_bytes",
                format!("exceeds the {MAX_DIFF_BYTES}-byte publication limit"),
            ));
        }
        if self.observed_branch_head != self.expected_branch_head {
            return Err(TriggerValidationError::field(
                "diff",
                "the observed branch head is stale",
            ));
        }
        let workflow_allowed = match launch
            .revision
            .content
            .repository_access
            .workflow_change_policy
        {
            WorkflowChangePolicy::Refuse => false,
            WorkflowChangePolicy::RequireScopeAndConfirmation => {
                launch.connection.as_ref().is_some_and(|connection| {
                    connection
                        .satisfied_scope_classes
                        .contains(&ScopeClass::Workflow)
                })
            }
        };
        for file in &self.files {
            if workflow_scope_required(file) && !workflow_allowed {
                return Err(TriggerValidationError::field(
                    "diff.files",
                    "workflow changes require the revision policy and observed workflow scope",
                ));
            }
        }
        if self.workflow_paths
            != self
                .files
                .iter()
                .filter(|file| file.path.starts_with(".github/workflows/"))
                .map(|file| file.path.clone())
                .collect::<Vec<_>>()
        {
            return Err(TriggerValidationError::field(
                "diff.workflow_paths",
                "does not match the reviewed path set",
            ));
        }
        Ok(())
    }
}

fn workflow_scope_required(file: &DiffFile) -> bool {
    file.path.starts_with(".github/workflows/")
        && (file.old_sha256.is_none()
            || file.new_sha256.is_none()
            || file.old_sha256 != file.new_sha256)
}

fn validate_optional_diff_mode(
    field: &str,
    mode: Option<u32>,
) -> Result<(), TriggerValidationError> {
    if mode.is_some_and(|mode| !matches!(mode, 0o100644 | 0o100755 | 0o120000)) {
        return Err(TriggerValidationError::field(
            field,
            "is not a supported regular-file or symlink mode",
        ));
    }
    Ok(())
}

fn validate_optional_object_id(
    field: &str,
    object: Option<&str>,
) -> Result<(), TriggerValidationError> {
    if object.is_some_and(|object| {
        object.len() != 40 || !object.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err(TriggerValidationError::field(
            field,
            "must be a 40-character hexadecimal Git object identifier",
        ));
    }
    Ok(())
}

fn validate_optional_sha256(
    field: &str,
    digest: Option<&str>,
) -> Result<(), TriggerValidationError> {
    if let Some(digest) = digest {
        validate_sha256(field, digest)?;
    }
    Ok(())
}

pub fn validate_workspace_path(path: &str) -> Result<(), TriggerValidationError> {
    workspace_path_alias(path).map(|_| ())
}

pub fn validate_workspace_path_aliases<'a>(
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<(), TriggerValidationError> {
    let mut aliases = BTreeSet::new();
    for path in paths {
        if !aliases.insert(workspace_path_alias(path)?) {
            return Err(TriggerValidationError::field(
                "workspace.entries",
                format!("{path} aliases another workspace path"),
            ));
        }
    }
    Ok(())
}

fn workspace_path_alias(path: &str) -> Result<String, TriggerValidationError> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
    {
        return Err(invalid_workspace_path());
    }
    let mut aliases = Vec::new();
    for segment in path.split('/') {
        let alias = segment
            .nfkc()
            .flat_map(char::to_lowercase)
            .collect::<String>();
        if segment.is_empty()
            || segment.chars().any(unsafe_path_character)
            || alias.is_empty()
            || matches!(alias.as_str(), "." | "..")
            || alias
                .chars()
                .any(|character| matches!(character, '/' | '\\' | ':'))
            || alias.ends_with('.')
            || alias.ends_with(' ')
            || reserved_workspace_segment(&alias)
        {
            return Err(invalid_workspace_path());
        }
        aliases.push(alias);
    }
    Ok(aliases.join("/"))
}

fn invalid_workspace_path() -> TriggerValidationError {
    TriggerValidationError::field("workspace.path", "is not a canonical relative path")
}

fn unsafe_path_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

fn reserved_workspace_segment(segment: &str) -> bool {
    if matches!(
        segment,
        ".git"
            | ".cma"
            | ".chrono"
            | ".nyxid"
            | ".ssh"
            | ".aws"
            | ".gnupg"
            | ".netrc"
            | ".npmrc"
            | ".pypirc"
            | "credentials"
            | "credentials.json"
            | "id_rsa"
            | "id_ed25519"
    ) || segment == ".env"
        || segment.starts_with(".env.")
        || segment.starts_with(".cma-")
        || segment.starts_with(".chrono-")
    {
        return true;
    }
    let stem = segment.split('.').next().unwrap_or(segment);
    matches!(stem, "con" | "prn" | "aux" | "nul" | "clock$")
        || stem
            .strip_prefix("com")
            .or_else(|| stem.strip_prefix("lpt"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

fn validate_symlink_target(path: &str, target: &str) -> Result<(), TriggerValidationError> {
    if target.is_empty()
        || target.len() > MAX_PATH_BYTES
        || target.starts_with('/')
        || target.contains('\\')
        || target.contains('\0')
    {
        return Err(TriggerValidationError::field(
            format!("workspace.entries.{path}.symlink_target"),
            "is not a safe relative target",
        ));
    }
    let mut depth = path.split('/').count().saturating_sub(1) as i64;
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return Err(TriggerValidationError::field(
                        format!("workspace.entries.{path}.symlink_target"),
                        "escapes the workspace",
                    ));
                }
            }
            _ => depth += 1,
        }
    }
    Ok(())
}

pub(crate) fn validate_sha256(field: &str, digest: &str) -> Result<(), TriggerValidationError> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TriggerValidationError::field(
            field,
            "must be a 64-character hexadecimal SHA-256 digest",
        ));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod submodule_tests {
    use super::*;

    fn entry() -> WorkspaceEntry {
        WorkspaceEntry {
            path: "vendor/library".into(),
            kind: WorkspaceEntryKind::Submodule,
            mode: 0o160000,
            size: 0,
            content_sha256: EMPTY_CONTENT_SHA256.into(),
            provider_object_id: Some("a".repeat(40)),
            symlink_target: None,
        }
    }

    fn regular(path: &str) -> WorkspaceEntry {
        WorkspaceEntry {
            path: path.into(),
            kind: WorkspaceEntryKind::Regular,
            mode: 0o100644,
            size: 1,
            content_sha256: "b".repeat(64),
            provider_object_id: Some("c".repeat(40)),
            symlink_target: None,
        }
    }

    #[test]
    fn submodule_entries_are_inert_and_pinned_to_a_commit() {
        entry().validate().expect("an inert gitlink is valid");

        let mut with_payload = entry();
        with_payload.size = 1;
        assert!(with_payload.validate().is_err());

        let mut blob_digest = entry();
        blob_digest.content_sha256 = "b".repeat(64);
        assert!(blob_digest.validate().is_err());

        let mut unpinned = entry();
        unpinned.provider_object_id = None;
        assert!(unpinned.validate().is_err());

        let mut invalid_object = entry();
        invalid_object.provider_object_id = Some("not-a-git-object".into());
        assert!(invalid_object.validate().is_err());
    }

    #[test]
    fn control_reserved_and_compatibility_paths_are_not_workspace_names() {
        for path in [
            "line\nbreak.txt",
            "bidi\u{202e}name.txt",
            "NUL.txt",
            "devices/COM1.log",
            "devices/lpt9",
            "trailing-dot.",
            "trailing-space ",
            "stream:name.txt",
            ".cma-control/runtime.json",
            "nested/.cma-control/config.toml",
            "fullwidth\u{ff0f}slash.txt",
        ] {
            assert!(
                validate_workspace_path(path).is_err(),
                "{path:?} must fail before it becomes an archive path"
            );
        }
        validate_workspace_path("docs/naive-\u{00ef}.md")
            .expect("an ordinary non-ASCII repository path remains supported");
    }

    #[test]
    fn case_unicode_and_file_directory_aliases_cannot_share_a_manifest() {
        for paths in [
            vec!["README.md", "readme.md"],
            vec!["caf\u{00e9}.txt", "cafe\u{0301}.txt"],
            vec!["Docs", "docs/guide.md"],
        ] {
            assert!(WorkspaceManifest::seal(
                "a".repeat(40),
                paths.into_iter().map(regular).collect(),
            )
            .is_err());
        }
    }
}
