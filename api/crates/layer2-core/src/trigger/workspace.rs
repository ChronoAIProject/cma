use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{CheckpointId, DiffArtifactId, LaunchId, TriggerValidationError};
use crate::SessionId;

pub const WORKSPACE_MANIFEST_VERSION: u32 = 1;
pub const MAX_REPOSITORY_FILES: usize = 100_000;
pub const MAX_REPOSITORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_PATH_BYTES: usize = 1_024;
pub const MAX_DIFF_FILES: usize = 2_000;
pub const MAX_DIFF_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceEntryKind {
    Regular,
    Symlink,
    Submodule,
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
                if self.mode != 0o160000 || self.symlink_target.is_some() {
                    return Err(TriggerValidationError::field(
                        format!("workspace.entries.{}", self.path),
                        "a submodule must use mode 160000 and no symlink target",
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
        let mut seen = BTreeSet::new();
        let mut total_bytes = 0_u64;
        for entry in &entries {
            entry.validate()?;
            if !seen.insert(entry.path.as_str()) {
                return Err(TriggerValidationError::field(
                    "workspace.entries",
                    format!("{} occurs more than once", entry.path),
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
    let bytes = serde_json::to_vec(&(WORKSPACE_MANIFEST_VERSION, source_revision, entries))
        .map_err(|_| TriggerValidationError::new("workspace manifest cannot be encoded"))?;
    Ok(hex(&Sha256::digest(bytes)))
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
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_after: Option<String>,
}

impl WorkspaceCheckpoint {
    pub fn validate_complete(&self, launch: &LaunchId) -> Result<(), TriggerValidationError> {
        if &self.launch_id != launch || self.workspace_epoch == 0 || self.version == 0 {
            return Err(TriggerValidationError::field(
                "checkpoint",
                "does not belong to the requested Launch epoch",
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
        self.manifest.validate()
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
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffValidationOutcome {
    Valid,
    NoChanges,
    StaleHead,
    WorkflowScopeRequired,
    TooManyChanges,
    UnsupportedFile,
    CorruptCheckpoint,
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
    pub files: Vec<DiffFile>,
    pub changed_bytes: u64,
    pub digest: String,
    pub validation: DiffValidationOutcome,
    #[serde(default)]
    pub checks: Vec<CheckResult>,
    pub created_at: String,
    pub delete_after: String,
}

impl DiffArtifact {
    pub fn validate_for_publication(
        &self,
        launch: &LaunchId,
    ) -> Result<(), TriggerValidationError> {
        if &self.launch_id != launch || self.validation != DiffValidationOutcome::Valid {
            return Err(TriggerValidationError::field(
                "diff",
                "is not a valid reviewed artifact for this Launch",
            ));
        }
        if self.files.is_empty() || self.files.len() > MAX_DIFF_FILES {
            return Err(TriggerValidationError::field(
                "diff.files",
                format!("must contain between one and {MAX_DIFF_FILES} changes"),
            ));
        }
        if self.changed_bytes > MAX_DIFF_BYTES {
            return Err(TriggerValidationError::field(
                "diff.changed_bytes",
                format!("exceeds the {MAX_DIFF_BYTES}-byte publication limit"),
            ));
        }
        validate_sha256("diff.baseline_digest", &self.baseline_digest)?;
        validate_sha256("diff.checkpoint_digest", &self.checkpoint_digest)?;
        validate_sha256("diff.digest", &self.digest)?;
        for file in &self.files {
            validate_workspace_path(&file.path)?;
            if file.path.starts_with(".github/workflows/") {
                return Err(TriggerValidationError::field(
                    "diff.files",
                    "workflow changes require the separately approved scope path",
                ));
            }
        }
        Ok(())
    }
}

pub fn validate_workspace_path(path: &str) -> Result<(), TriggerValidationError> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(TriggerValidationError::field(
            "workspace.path",
            "is not a canonical relative path",
        ));
    }
    Ok(())
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
