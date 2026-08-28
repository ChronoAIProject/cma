//! Deterministic, byte-verified review artifacts for Trigger contributions.

use std::collections::{BTreeMap, BTreeSet};

use layer2_core::{
    CommitIdentity, DiffArtifact, DiffArtifactId, DiffChangeKind, DiffFile, DiffValidationOutcome,
    Launch, ScopeClass, WorkflowChangePolicy, WorkspaceCheckpoint, WorkspaceEntry,
    WorkspaceEntryKind, WorkspaceManifest, MAX_DIFF_ARTIFACT_BYTES, MAX_DIFF_BYTES, MAX_DIFF_FILES,
    MAX_DIFF_SUMMARY_FILES, MAX_DIFF_SUMMARY_PATCH_BYTES, MAX_REPOSITORY_BYTES,
};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest as _, Sha256};
use similar::TextDiff;

const WORKSPACE_ARCHIVE_VERSION: u32 = 1;
const MAX_WORKSPACE_HEADER_BYTES: usize = 128 * 1024 * 1024;
const MAX_PATCH_INPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_PATCH_BYTES_PER_FILE: usize = 1024 * 1024;
const MAX_COMPLETE_PATCH_BYTES: usize = 48 * 1024 * 1024;
const MAX_SUMMARY_PATCH_BYTES_PER_FILE: usize = 8 * 1024;
pub(crate) const CMA_COMMITTER_NAME: &str = "CMA";
pub(crate) const CMA_COMMITTER_EMAIL: &str = "cma@chrono.ai";

#[derive(Debug, thiserror::Error)]
pub(crate) enum DiffBuildError {
    #[error("workspace archive is corrupt: {0}")]
    Corrupt(String),
    #[error("the complete diff artifact exceeds its durable object bound")]
    ArtifactTooLarge,
    #[error("diff artifact cannot be represented: {0}")]
    Invalid(String),
}

#[derive(Debug)]
pub(crate) struct BuiltDiffArtifact {
    pub(crate) artifact: DiffArtifact,
    pub(crate) object: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewDocument {
    cma_trigger_diff: u32,
    artifact_id: DiffArtifactId,
    launch_id: layer2_core::LaunchId,
    checkpoint_id: layer2_core::CheckpointId,
    baseline_digest: String,
    checkpoint_digest: String,
    expected_branch_head: String,
    observed_branch_head: String,
    changed_file_limit: u32,
    total_changed_files: u32,
    changed_bytes: u64,
    validation: DiffValidationOutcome,
    author: CommitIdentity,
    committer: CommitIdentity,
    workflow_paths: Vec<String>,
    files: Vec<DiffFile>,
    created_at: String,
    delete_after: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublicationPlan {
    pub(crate) launch_id: layer2_core::LaunchId,
    pub(crate) artifact_id: DiffArtifactId,
    pub(crate) revision_digest: String,
    pub(crate) baseline_digest: String,
    pub(crate) checkpoint_digest: String,
    pub(crate) diff_digest: String,
    pub(crate) expected_head: String,
    pub(crate) author: CommitIdentity,
    pub(crate) committer: CommitIdentity,
    pub(crate) files: Vec<PublicationFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublicationFile {
    pub(crate) path: String,
    pub(crate) change: DiffChangeKind,
    pub(crate) binary: bool,
    pub(crate) mode: String,
    pub(crate) content_sha256: Option<String>,
    pub(crate) blob_id: Option<String>,
    pub(crate) content: Option<Vec<u8>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveHeader {
    cma_workspace: u32,
    workspace_epoch: u64,
    #[serde(default)]
    checkpoint_version: u64,
    #[serde(default)]
    quiesced_writers: u32,
    source_revision: String,
    manifest_digest: String,
    entries: Vec<ArchiveEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveEntry {
    path: String,
    kind: WorkspaceEntryKind,
    mode: u32,
    size: u64,
    content_sha256: String,
    provider_object_id: Option<String>,
    offset: u64,
}

struct OpenedEntry<'a> {
    metadata: &'a WorkspaceEntry,
    bytes: &'a [u8],
}

pub(crate) fn deterministic_diff_id(intent_key: &str) -> DiffArtifactId {
    let digest = Sha256::digest(format!("cma-diff-v1\0{intent_key}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    DiffArtifactId(uuid::Uuid::from_bytes(bytes).to_string())
}

pub(crate) fn build_publication_plan(
    launch: &Launch,
    artifact: &DiffArtifact,
    artifact_object: &[u8],
    checkpoint_archive: &[u8],
) -> Result<PublicationPlan, DiffBuildError> {
    artifact
        .validate_for_review(launch)
        .map_err(|error| DiffBuildError::Invalid(error.to_string()))?;
    if usize::try_from(artifact.changed_file_limit).unwrap_or(usize::MAX) > MAX_DIFF_SUMMARY_FILES {
        return Err(DiffBuildError::Invalid(
            "the measured deployment publication ceiling exceeds the complete validation bound"
                .into(),
        ));
    }
    if u64::try_from(artifact_object.len()).unwrap_or(u64::MAX) != artifact.object_bytes
        || hex(&Sha256::digest(artifact_object)) != artifact.object_sha256
    {
        return Err(DiffBuildError::Corrupt(
            "the retained review object failed its size or SHA-256".into(),
        ));
    }
    let document: ReviewDocument = serde_json::from_slice(artifact_object)
        .map_err(|_| DiffBuildError::Corrupt("the review object is malformed".into()))?;
    if serde_json::to_vec(&document).ok().as_deref() != Some(artifact_object) {
        return Err(DiffBuildError::Corrupt(
            "the review object is not in its canonical byte form".into(),
        ));
    }
    if document.cma_trigger_diff != 1
        || document.artifact_id != artifact.id
        || document.launch_id != artifact.launch_id
        || document.checkpoint_id != artifact.checkpoint_id
        || document.baseline_digest != artifact.baseline_digest
        || document.checkpoint_digest != artifact.checkpoint_digest
        || document.expected_branch_head != artifact.expected_branch_head
        || document.observed_branch_head != artifact.observed_branch_head
        || document.changed_file_limit != artifact.changed_file_limit
        || document.total_changed_files != artifact.total_changed_files
        || document.changed_bytes != artifact.changed_bytes
        || document.validation != artifact.validation
        || document.author != artifact.author
        || document.committer != artifact.committer
        || document.workflow_paths != artifact.workflow_paths
        || document.created_at != artifact.created_at
        || document.delete_after != artifact.delete_after
        || usize::try_from(document.total_changed_files).ok() != Some(document.files.len())
    {
        return Err(DiffBuildError::Corrupt(
            "the review object disagrees with its retained Launch summary".into(),
        ));
    }
    let mut complete = artifact.clone();
    complete.files = document.files.clone();
    complete
        .validate_for_publication(launch)
        .map_err(|error| DiffBuildError::Invalid(error.to_string()))?;

    let workspace = launch
        .workspace
        .as_ref()
        .ok_or_else(|| DiffBuildError::Invalid("Launch workspace is missing".into()))?;
    let checkpoint = workspace
        .latest_checkpoint
        .as_ref()
        .filter(|checkpoint| checkpoint.id == artifact.checkpoint_id)
        .ok_or_else(|| DiffBuildError::Invalid("review checkpoint is missing".into()))?;
    workspace
        .baseline
        .validate()
        .map_err(|error| DiffBuildError::Corrupt(error.to_string()))?;
    checkpoint
        .validate_complete(&launch.id)
        .map_err(|error| DiffBuildError::Corrupt(error.to_string()))?;
    let current = open_archive(
        checkpoint_archive,
        &checkpoint.manifest,
        checkpoint.workspace_epoch,
        checkpoint.version,
        &checkpoint.archive_sha256,
    )?;

    let mut files = Vec::with_capacity(document.files.len());
    for reviewed in document.files {
        let current_entry = current.get(reviewed.path.as_str());
        let publication = match reviewed.new_sha256.as_deref() {
            Some(new_sha256) => {
                let current_entry = current_entry.ok_or_else(|| {
                    DiffBuildError::Corrupt(format!(
                        "reviewed path {} is absent from the checkpoint",
                        reviewed.path
                    ))
                })?;
                if current_entry.metadata.content_sha256 != new_sha256
                    || Some(current_entry.metadata.mode) != reviewed.new_mode
                    || current_entry.metadata.size != reviewed.new_bytes
                    || current_entry.metadata.provider_object_id != reviewed.new_provider_object_id
                {
                    return Err(DiffBuildError::Corrupt(format!(
                        "reviewed path {} disagrees with the checkpoint",
                        reviewed.path
                    )));
                }
                let blob_id = git_blob_id(current_entry.bytes);
                if current_entry
                    .metadata
                    .provider_object_id
                    .as_ref()
                    .is_some_and(|provider| !provider.eq_ignore_ascii_case(&blob_id))
                {
                    return Err(DiffBuildError::Corrupt(format!(
                        "reviewed path {} has a mismatched Git blob identity",
                        reviewed.path
                    )));
                }
                PublicationFile {
                    path: reviewed.path,
                    change: reviewed.change,
                    binary: reviewed.binary,
                    mode: git_mode(current_entry.metadata.mode)?.into(),
                    content_sha256: Some(new_sha256.to_string()),
                    blob_id: Some(blob_id),
                    content: Some(current_entry.bytes.to_vec()),
                }
            }
            None => {
                if current_entry.is_some() || reviewed.change != DiffChangeKind::Deleted {
                    return Err(DiffBuildError::Corrupt(format!(
                        "reviewed deletion {} disagrees with the checkpoint",
                        reviewed.path
                    )));
                }
                PublicationFile {
                    path: reviewed.path,
                    change: reviewed.change,
                    binary: reviewed.binary,
                    mode: git_mode(reviewed.old_mode.ok_or_else(|| {
                        DiffBuildError::Corrupt("a reviewed deletion has no prior mode".into())
                    })?)?
                    .into(),
                    content_sha256: None,
                    blob_id: None,
                    content: None,
                }
            }
        };
        files.push(publication);
    }

    Ok(PublicationPlan {
        launch_id: launch.id.clone(),
        artifact_id: artifact.id.clone(),
        revision_digest: launch.revision_digest.clone(),
        baseline_digest: artifact.baseline_digest.clone(),
        checkpoint_digest: artifact.checkpoint_digest.clone(),
        diff_digest: artifact.digest.clone(),
        expected_head: artifact.expected_branch_head.clone(),
        author: artifact.author.clone(),
        committer: artifact.committer.clone(),
        files,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_diff_artifact(
    launch: &Launch,
    checkpoint: &WorkspaceCheckpoint,
    baseline_archive: &[u8],
    checkpoint_archive: &[u8],
    observed_branch_head: &str,
    changed_file_limit: u32,
    artifact_id: DiffArtifactId,
    created_at: String,
) -> Result<BuiltDiffArtifact, DiffBuildError> {
    let workspace = launch
        .workspace
        .as_ref()
        .ok_or_else(|| DiffBuildError::Invalid("Launch workspace is missing".into()))?;
    if changed_file_limit == 0
        || usize::try_from(changed_file_limit).unwrap_or(usize::MAX) > MAX_DIFF_FILES
    {
        return Err(DiffBuildError::Invalid(
            "changed-file limit is outside the schema safety ceiling".into(),
        ));
    }
    workspace
        .baseline
        .validate()
        .map_err(|error| DiffBuildError::Corrupt(error.to_string()))?;
    checkpoint
        .validate_complete(&launch.id)
        .map_err(|error| DiffBuildError::Corrupt(error.to_string()))?;
    if checkpoint.manifest.source_revision != launch.source_sha
        || workspace.baseline.source_revision != launch.source_sha
        || checkpoint.workspace_epoch != workspace.workspace_epoch
        || checkpoint.delete_after.as_deref() != Some(workspace.delete_after.as_str())
    {
        return Err(DiffBuildError::Corrupt(
            "baseline and checkpoint authority do not match the Launch".into(),
        ));
    }
    if workspace
        .baseline
        .entries
        .iter()
        .chain(&checkpoint.manifest.entries)
        .any(|entry| entry.kind == WorkspaceEntryKind::Submodule)
    {
        return build_invalid_diff_artifact(
            launch,
            checkpoint,
            observed_branch_head,
            changed_file_limit,
            artifact_id,
            created_at,
            DiffValidationOutcome::UnsupportedFile,
        );
    }

    let baseline = open_archive(
        baseline_archive,
        &workspace.baseline,
        workspace.workspace_epoch,
        0,
        &workspace.baseline_archive_sha256,
    )?;
    let current = open_archive(
        checkpoint_archive,
        &checkpoint.manifest,
        checkpoint.workspace_epoch,
        checkpoint.version,
        &checkpoint.archive_sha256,
    )?;

    let mut paths = BTreeSet::new();
    paths.extend(baseline.keys().copied());
    paths.extend(current.keys().copied());
    let mut files = Vec::new();
    let mut changed_bytes = 0_u64;
    let mut complete_patch_bytes = 0_usize;
    for path in paths {
        let old = baseline.get(path);
        let new = current.get(path);
        if old.is_some_and(|old| new.is_some_and(|new| entries_equal(old.metadata, new.metadata))) {
            continue;
        }
        let old_size = old.map_or(0, |entry| entry.metadata.size);
        let new_size = new.map_or(0, |entry| entry.metadata.size);
        changed_bytes = changed_bytes
            .checked_add(old_size.max(new_size))
            .ok_or_else(|| DiffBuildError::Invalid("changed byte count overflowed".into()))?;
        let binary = old.is_some_and(|entry| is_binary(entry.bytes))
            || new.is_some_and(|entry| is_binary(entry.bytes));
        let (mut patch, mut patch_complete) = build_patch(old, new, binary);
        if let Some(rendered) = patch.as_ref() {
            let next = complete_patch_bytes.saturating_add(rendered.len());
            if next > MAX_COMPLETE_PATCH_BYTES {
                patch = None;
                patch_complete = false;
            } else {
                complete_patch_bytes = next;
            }
        }
        files.push(DiffFile {
            path: path.to_string(),
            change: change_kind(
                old.map(|entry| entry.metadata),
                new.map(|entry| entry.metadata),
            ),
            binary,
            old_mode: old.map(|entry| entry.metadata.mode),
            new_mode: new.map(|entry| entry.metadata.mode),
            old_sha256: old.map(|entry| entry.metadata.content_sha256.clone()),
            new_sha256: new.map(|entry| entry.metadata.content_sha256.clone()),
            old_provider_object_id: old.and_then(|entry| entry.metadata.provider_object_id.clone()),
            new_provider_object_id: new.and_then(|entry| entry.metadata.provider_object_id.clone()),
            old_bytes: old_size,
            new_bytes: new_size,
            patch,
            patch_complete,
        });
    }

    let total_changed_files = u32::try_from(files.len()).unwrap_or(u32::MAX);
    let workflow_paths = files
        .iter()
        .filter(|file| file.path.starts_with(".github/workflows/"))
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let workflow_scope_paths = files
        .iter()
        .filter(|file| {
            file.path.starts_with(".github/workflows/")
                && (file.old_sha256.is_none()
                    || file.new_sha256.is_none()
                    || file.old_sha256 != file.new_sha256)
        })
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let validation = validation_outcome(
        launch,
        observed_branch_head,
        files.len(),
        changed_bytes,
        &workflow_scope_paths,
        changed_file_limit,
    );
    if files.len() > MAX_DIFF_FILES {
        files.truncate(MAX_DIFF_FILES);
    }

    match seal_diff_artifact(
        launch,
        checkpoint,
        observed_branch_head,
        changed_file_limit,
        artifact_id.clone(),
        total_changed_files,
        changed_bytes,
        validation,
        workflow_paths,
        files,
        created_at.clone(),
    ) {
        Err(DiffBuildError::ArtifactTooLarge) => build_invalid_diff_artifact(
            launch,
            checkpoint,
            observed_branch_head,
            changed_file_limit,
            artifact_id,
            created_at,
            DiffValidationOutcome::ArtifactTooLarge,
        ),
        result => result,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_invalid_diff_artifact(
    launch: &Launch,
    checkpoint: &WorkspaceCheckpoint,
    observed_branch_head: &str,
    changed_file_limit: u32,
    artifact_id: DiffArtifactId,
    created_at: String,
    validation: DiffValidationOutcome,
) -> Result<BuiltDiffArtifact, DiffBuildError> {
    if matches!(validation, DiffValidationOutcome::Valid) {
        return Err(DiffBuildError::Invalid(
            "an invalid-artifact builder cannot grant publication authority".into(),
        ));
    }
    seal_diff_artifact(
        launch,
        checkpoint,
        observed_branch_head,
        changed_file_limit,
        artifact_id,
        0,
        0,
        validation,
        Vec::new(),
        Vec::new(),
        created_at,
    )
}

#[allow(clippy::too_many_arguments)]
fn seal_diff_artifact(
    launch: &Launch,
    checkpoint: &WorkspaceCheckpoint,
    observed_branch_head: &str,
    changed_file_limit: u32,
    artifact_id: DiffArtifactId,
    total_changed_files: u32,
    changed_bytes: u64,
    validation: DiffValidationOutcome,
    workflow_paths: Vec<String>,
    files: Vec<DiffFile>,
    created_at: String,
) -> Result<BuiltDiffArtifact, DiffBuildError> {
    let workspace = launch
        .workspace
        .as_ref()
        .ok_or_else(|| DiffBuildError::Invalid("Launch workspace is missing".into()))?;
    if changed_file_limit == 0
        || usize::try_from(changed_file_limit).unwrap_or(usize::MAX) > MAX_DIFF_FILES
    {
        return Err(DiffBuildError::Invalid(
            "changed-file limit is outside the schema safety ceiling".into(),
        ));
    }
    let expected_branch_head = &launch
        .branch
        .as_ref()
        .ok_or_else(|| DiffBuildError::Invalid("Launch branch is missing".into()))?
        .current_head;
    let author = contributor_identity(launch)?;
    let committer = CommitIdentity {
        name: CMA_COMMITTER_NAME.into(),
        email: CMA_COMMITTER_EMAIL.into(),
    };
    author
        .validate()
        .map_err(|error| DiffBuildError::Invalid(error.to_string()))?;
    committer
        .validate()
        .map_err(|error| DiffBuildError::Invalid(error.to_string()))?;

    let document = ReviewDocument {
        cma_trigger_diff: 1,
        artifact_id: artifact_id.clone(),
        launch_id: launch.id.clone(),
        checkpoint_id: checkpoint.id.clone(),
        baseline_digest: workspace.baseline.digest.clone(),
        checkpoint_digest: checkpoint.manifest.digest.clone(),
        expected_branch_head: expected_branch_head.clone(),
        observed_branch_head: observed_branch_head.to_string(),
        changed_file_limit,
        total_changed_files,
        changed_bytes,
        validation,
        author: author.clone(),
        committer: committer.clone(),
        workflow_paths: workflow_paths.clone(),
        files: files.clone(),
        created_at: created_at.clone(),
        delete_after: workspace.delete_after.clone(),
    };
    let object = serde_json::to_vec(&document)
        .map_err(|error| DiffBuildError::Invalid(format!("encode review artifact: {error}")))?;
    if object.is_empty()
        || u64::try_from(object.len()).unwrap_or(u64::MAX) > MAX_DIFF_ARTIFACT_BYTES
    {
        return Err(DiffBuildError::ArtifactTooLarge);
    }
    let object_sha256 = hex(&Sha256::digest(&object));
    let summary_files = bounded_summary(&files);
    Ok(BuiltDiffArtifact {
        artifact: DiffArtifact {
            id: artifact_id,
            launch_id: launch.id.clone(),
            checkpoint_id: checkpoint.id.clone(),
            baseline_digest: workspace.baseline.digest.clone(),
            checkpoint_digest: checkpoint.manifest.digest.clone(),
            expected_branch_head: expected_branch_head.clone(),
            observed_branch_head: observed_branch_head.to_string(),
            changed_file_limit,
            total_changed_files,
            files: summary_files,
            changed_bytes,
            digest: object_sha256.clone(),
            object_reference: String::new(),
            object_sha256,
            object_bytes: u64::try_from(object.len()).unwrap_or(u64::MAX),
            author,
            committer,
            workflow_paths,
            validation,
            checks: Vec::new(),
            created_at,
            delete_after: workspace.delete_after.clone(),
        },
        object,
    })
}

fn validation_outcome(
    launch: &Launch,
    observed_branch_head: &str,
    changed_files: usize,
    changed_bytes: u64,
    workflow_scope_paths: &[String],
    changed_file_limit: u32,
) -> DiffValidationOutcome {
    if launch
        .branch
        .as_ref()
        .is_none_or(|branch| branch.current_head != observed_branch_head)
    {
        return DiffValidationOutcome::StaleHead;
    }
    if changed_files == 0 {
        return DiffValidationOutcome::NoChanges;
    }
    if changed_files > usize::try_from(changed_file_limit).unwrap_or(usize::MAX) {
        return DiffValidationOutcome::TooManyChanges;
    }
    if changed_bytes > MAX_DIFF_BYTES {
        return DiffValidationOutcome::TooManyBytes;
    }
    if !workflow_scope_paths.is_empty() {
        let allowed = launch
            .revision
            .content
            .repository_access
            .workflow_change_policy
            == WorkflowChangePolicy::RequireScopeAndConfirmation
            && launch.connection.as_ref().is_some_and(|connection| {
                connection
                    .satisfied_scope_classes
                    .contains(&ScopeClass::Workflow)
            });
        if !allowed {
            return DiffValidationOutcome::WorkflowScopeRequired;
        }
    }
    DiffValidationOutcome::Valid
}

fn contributor_identity(launch: &Launch) -> Result<CommitIdentity, DiffBuildError> {
    let account = launch
        .connection
        .as_ref()
        .map(|connection| &connection.account)
        .ok_or_else(|| DiffBuildError::Invalid("GitHub contributor identity is missing".into()))?;
    if account.login.is_empty()
        || account.login.len() > 39
        || !account
            .login
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(DiffBuildError::Invalid(
            "GitHub contributor login is outside the public identity policy".into(),
        ));
    }
    Ok(CommitIdentity {
        name: account.login.clone(),
        email: format!(
            "{}+{}@users.noreply.github.com",
            account.account_id, account.login
        ),
    })
}

fn entries_equal(old: &WorkspaceEntry, new: &WorkspaceEntry) -> bool {
    old.kind == new.kind
        && old.mode == new.mode
        && old.content_sha256 == new.content_sha256
        && old.provider_object_id == new.provider_object_id
        && old.symlink_target == new.symlink_target
}

fn change_kind(old: Option<&WorkspaceEntry>, new: Option<&WorkspaceEntry>) -> DiffChangeKind {
    match (old, new) {
        (None, Some(_)) => DiffChangeKind::Added,
        (Some(_), None) => DiffChangeKind::Deleted,
        (Some(old), Some(new))
            if old.content_sha256 == new.content_sha256
                && old.kind == new.kind
                && old.symlink_target == new.symlink_target =>
        {
            DiffChangeKind::ModeChanged
        }
        (Some(old), Some(new))
            if old.kind == WorkspaceEntryKind::Symlink
                || new.kind == WorkspaceEntryKind::Symlink =>
        {
            DiffChangeKind::SymlinkChanged
        }
        (Some(_), Some(_)) => DiffChangeKind::Modified,
        (None, None) => unreachable!("a union path has at least one entry"),
    }
}

fn build_patch(
    old: Option<&OpenedEntry<'_>>,
    new: Option<&OpenedEntry<'_>>,
    binary: bool,
) -> (Option<String>, bool) {
    let old_bytes = old.map_or(&[][..], |entry| entry.bytes);
    let new_bytes = new.map_or(&[][..], |entry| entry.bytes);
    if old_bytes == new_bytes {
        return (None, true);
    }
    if binary || old_bytes.len().saturating_add(new_bytes.len()) > MAX_PATCH_INPUT_BYTES {
        return (None, false);
    }
    let (Ok(old_text), Ok(new_text)) = (
        std::str::from_utf8(old_bytes),
        std::str::from_utf8(new_bytes),
    ) else {
        return (None, false);
    };
    let patch = TextDiff::from_lines(old_text, new_text)
        .unified_diff()
        .context_radius(3)
        .header("baseline", "checkpoint")
        .to_string();
    if patch.len() > MAX_PATCH_BYTES_PER_FILE {
        (None, false)
    } else {
        (Some(patch), true)
    }
}

fn bounded_summary(files: &[DiffFile]) -> Vec<DiffFile> {
    let mut retained = Vec::with_capacity(files.len().min(MAX_DIFF_SUMMARY_FILES));
    let mut patch_bytes = 0_usize;
    for file in files.iter().take(MAX_DIFF_SUMMARY_FILES) {
        let mut summary = file.clone();
        if summary.patch.as_ref().is_some_and(|patch| {
            patch.len() > MAX_SUMMARY_PATCH_BYTES_PER_FILE
                || patch_bytes.saturating_add(patch.len()) > MAX_DIFF_SUMMARY_PATCH_BYTES
        }) {
            summary.patch = None;
            summary.patch_complete = false;
        } else if let Some(patch) = summary.patch.as_ref() {
            patch_bytes += patch.len();
        }
        retained.push(summary);
    }
    retained
}

pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0) || std::str::from_utf8(bytes).is_err()
}

fn open_archive<'a>(
    archive: &'a [u8],
    manifest: &'a WorkspaceManifest,
    expected_epoch: u64,
    expected_checkpoint_version: u64,
    expected_archive_sha256: &str,
) -> Result<BTreeMap<&'a str, OpenedEntry<'a>>, DiffBuildError> {
    let max_archive = MAX_REPOSITORY_BYTES
        .saturating_add(u64::try_from(MAX_WORKSPACE_HEADER_BYTES).unwrap_or(u64::MAX))
        .saturating_add(8);
    if archive.is_empty()
        || u64::try_from(archive.len()).unwrap_or(u64::MAX) > max_archive
        || hex(&Sha256::digest(archive)) != expected_archive_sha256
    {
        return Err(DiffBuildError::Corrupt(
            "archive length or SHA-256 is invalid".into(),
        ));
    }
    let prefix: [u8; 8] = archive
        .get(..8)
        .ok_or_else(|| DiffBuildError::Corrupt("archive has no header length".into()))?
        .try_into()
        .map_err(|_| DiffBuildError::Corrupt("archive header length is invalid".into()))?;
    let header_len = usize::try_from(u64::from_be_bytes(prefix))
        .map_err(|_| DiffBuildError::Corrupt("archive header length overflows".into()))?;
    if header_len == 0 || header_len > MAX_WORKSPACE_HEADER_BYTES {
        return Err(DiffBuildError::Corrupt(
            "archive header is outside its bound".into(),
        ));
    }
    let header_end = 8_usize
        .checked_add(header_len)
        .ok_or_else(|| DiffBuildError::Corrupt("archive header length overflows".into()))?;
    let header: ArchiveHeader = serde_json::from_slice(
        archive
            .get(8..header_end)
            .ok_or_else(|| DiffBuildError::Corrupt("archive header is truncated".into()))?,
    )
    .map_err(|_| DiffBuildError::Corrupt("archive header is malformed".into()))?;
    let payload = archive
        .get(header_end..)
        .ok_or_else(|| DiffBuildError::Corrupt("archive payload is missing".into()))?;
    if header.cma_workspace != WORKSPACE_ARCHIVE_VERSION
        || header.workspace_epoch != expected_epoch
        || header.checkpoint_version != expected_checkpoint_version
        || (expected_checkpoint_version == 0 && header.quiesced_writers != 0)
        || header.source_revision != manifest.source_revision
        || header.manifest_digest != manifest.digest
        || header.entries.len() != manifest.entries.len()
    {
        return Err(DiffBuildError::Corrupt(
            "archive authority does not match its manifest".into(),
        ));
    }

    let mut expected_offset = 0_u64;
    let mut result = BTreeMap::new();
    for (wire, metadata) in header.entries.iter().zip(&manifest.entries) {
        if wire.path != metadata.path
            || wire.kind != metadata.kind
            || wire.mode != metadata.mode
            || wire.size != metadata.size
            || wire.content_sha256 != metadata.content_sha256
            || wire.provider_object_id != metadata.provider_object_id
            || wire.offset != expected_offset
        {
            return Err(DiffBuildError::Corrupt(
                "archive entry metadata disagrees with its manifest".into(),
            ));
        }
        expected_offset = expected_offset
            .checked_add(wire.size)
            .ok_or_else(|| DiffBuildError::Corrupt("archive payload size overflowed".into()))?;
        let start = usize::try_from(wire.offset)
            .map_err(|_| DiffBuildError::Corrupt("archive entry offset overflows".into()))?;
        let end = usize::try_from(expected_offset)
            .map_err(|_| DiffBuildError::Corrupt("archive entry size overflows".into()))?;
        let bytes = payload
            .get(start..end)
            .ok_or_else(|| DiffBuildError::Corrupt("archive entry is truncated".into()))?;
        if hex(&Sha256::digest(bytes)) != wire.content_sha256
            || wire
                .provider_object_id
                .as_deref()
                .is_some_and(|object| git_blob_id(bytes) != object.to_ascii_lowercase())
            || (wire.kind == WorkspaceEntryKind::Symlink
                && std::str::from_utf8(bytes).ok() != metadata.symlink_target.as_deref())
        {
            return Err(DiffBuildError::Corrupt(
                "archive entry bytes failed integrity verification".into(),
            ));
        }
        if result
            .insert(metadata.path.as_str(), OpenedEntry { metadata, bytes })
            .is_some()
        {
            return Err(DiffBuildError::Corrupt(
                "archive repeats a workspace path".into(),
            ));
        }
    }
    if usize::try_from(expected_offset).ok() != Some(payload.len()) {
        return Err(DiffBuildError::Corrupt(
            "archive payload contains a gap or trailing bytes".into(),
        ));
    }
    Ok(result)
}

pub(crate) fn git_blob_id(bytes: &[u8]) -> String {
    let mut digest = Sha1::new();
    digest.update(format!("blob {}\0", bytes.len()).as_bytes());
    digest.update(bytes);
    hex(&digest.finalize())
}

fn git_mode(mode: u32) -> Result<&'static str, DiffBuildError> {
    match mode {
        0o100644 => Ok("100644"),
        0o100755 => Ok("100755"),
        0o120000 => Ok("120000"),
        _ => Err(DiffBuildError::Invalid(
            "the publication plan contains an unsupported Git mode".into(),
        )),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use layer2_core::{
        BranchRecord, CheckpointId, CheckpointIntegrity, DiffChangeKind, SessionId, WorkspaceRecord,
    };

    use super::*;

    const NOW: &str = "2026-08-27T00:00:00Z";
    const DELETE_AFTER: &str = "2026-09-27T00:00:00Z";

    #[derive(Clone)]
    struct EntryFixture {
        path: String,
        kind: WorkspaceEntryKind,
        mode: u32,
        bytes: Vec<u8>,
        provider_object_id: Option<String>,
        symlink_target: Option<String>,
    }

    struct DiffFixture {
        launch: Launch,
        baseline_archive: Vec<u8>,
        checkpoint_archive: Vec<u8>,
    }

    #[test]
    fn golden_diff_covers_every_supported_file_state_and_is_deterministic() {
        let fixture = fixture(
            vec![
                regular("binary.dat", 0o100644, b"old\0binary"),
                regular("deleted.bin", 0o100644, b"delete\0me"),
                symlink("link", "old-target"),
                regular("mode.sh", 0o100644, b"echo stable\n"),
                regular("modified.txt", 0o100644, b"before\n"),
            ],
            vec![
                regular("added.txt", 0o100644, b"added\n"),
                regular("binary.dat", 0o100644, b"new\0binary"),
                symlink("link", "new-target"),
                regular("mode.sh", 0o100755, b"echo stable\n"),
                regular("modified.txt", 0o100644, b"after\n"),
            ],
        );
        let first = build(
            &fixture,
            &fixture.launch.branch.as_ref().unwrap().current_head,
            1_000,
        )
        .expect("the golden diff builds");
        let second = build(
            &fixture,
            &fixture.launch.branch.as_ref().unwrap().current_head,
            1_000,
        )
        .expect("the golden diff repeats");

        assert_eq!(first.object, second.object);
        assert_eq!(first.artifact.digest, second.artifact.digest);
        assert_eq!(first.artifact.validation, DiffValidationOutcome::Valid);
        assert_eq!(first.artifact.total_changed_files, 6);
        assert_eq!(first.artifact.author.name, "contributor");
        assert_eq!(
            first.artifact.author.email,
            "21+contributor@users.noreply.github.com"
        );
        assert_eq!(first.artifact.committer.name, CMA_COMMITTER_NAME);
        assert_eq!(first.artifact.committer.email, CMA_COMMITTER_EMAIL);

        let files = first
            .artifact
            .files
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(files["added.txt"].change, DiffChangeKind::Added);
        assert!(files["added.txt"]
            .patch
            .as_deref()
            .unwrap()
            .contains("+added"));
        assert_eq!(files["modified.txt"].change, DiffChangeKind::Modified);
        assert!(files["modified.txt"]
            .patch
            .as_deref()
            .unwrap()
            .contains("-before"));
        assert_eq!(files["deleted.bin"].change, DiffChangeKind::Deleted);
        assert!(files["deleted.bin"].binary);
        assert!(!files["deleted.bin"].patch_complete);
        assert_eq!(files["binary.dat"].change, DiffChangeKind::Modified);
        assert!(files["binary.dat"].binary);
        assert!(files["binary.dat"].patch.is_none());
        assert_eq!(files["mode.sh"].change, DiffChangeKind::ModeChanged);
        assert_eq!(files["mode.sh"].old_sha256, files["mode.sh"].new_sha256);
        assert!(files["mode.sh"].patch_complete);
        assert_eq!(files["link"].change, DiffChangeKind::SymlinkChanged);
        assert!(files["link"]
            .patch
            .as_deref()
            .unwrap()
            .contains("new-target"));

        let document: serde_json::Value = serde_json::from_slice(&first.object).unwrap();
        assert_eq!(document["validation"], "valid");
        assert_eq!(document["files"].as_array().unwrap().len(), 6);
        assert_eq!(
            hex(&Sha256::digest(&first.object)),
            first.artifact.object_sha256
        );

        let mut retained = first.artifact;
        retained.object_reference = "test/diff-object.json".into();
        retained.validate_for_review(&fixture.launch).unwrap();
        retained.validate_for_publication(&fixture.launch).unwrap();
    }

    #[test]
    fn no_changes_and_stale_head_are_named_review_outcomes() {
        let unchanged = vec![regular("README.md", 0o100644, b"same\n")];
        let unchanged_fixture = fixture(unchanged.clone(), unchanged);
        let mut no_changes = build(
            &unchanged_fixture,
            &unchanged_fixture
                .launch
                .branch
                .as_ref()
                .unwrap()
                .current_head,
            10,
        )
        .unwrap()
        .artifact;
        no_changes.object_reference = "test/no-changes.json".into();
        assert_eq!(no_changes.validation, DiffValidationOutcome::NoChanges);
        no_changes
            .validate_for_review(&unchanged_fixture.launch)
            .unwrap();
        assert!(no_changes
            .validate_for_publication(&unchanged_fixture.launch)
            .is_err());

        let changed = fixture(
            vec![regular("README.md", 0o100644, b"old\n")],
            vec![regular("README.md", 0o100644, b"new\n")],
        );
        let mut stale = build(&changed, &"f".repeat(40), 10).unwrap().artifact;
        stale.object_reference = "test/stale.json".into();
        assert_eq!(stale.validation, DiffValidationOutcome::StaleHead);
        stale.validate_for_review(&changed.launch).unwrap();
        assert!(stale.validate_for_publication(&changed.launch).is_err());
    }

    #[test]
    fn workflow_scope_is_checked_at_review_with_same_content_exception() {
        let path = ".github/workflows/check.yml";
        let changed = fixture(
            vec![regular(path, 0o100644, b"name: old\n")],
            vec![regular(path, 0o100644, b"name: new\n")],
        );
        let refused = build(
            &changed,
            &changed.launch.branch.as_ref().unwrap().current_head,
            10,
        )
        .unwrap();
        assert_eq!(
            refused.artifact.validation,
            DiffValidationOutcome::WorkflowScopeRequired
        );
        assert_eq!(refused.artifact.workflow_paths, [path]);

        let mode_only = fixture(
            vec![regular(path, 0o100644, b"name: stable\n")],
            vec![regular(path, 0o100755, b"name: stable\n")],
        );
        assert_eq!(
            build(
                &mode_only,
                &mode_only.launch.branch.as_ref().unwrap().current_head,
                10,
            )
            .unwrap()
            .artifact
            .validation,
            DiffValidationOutcome::Valid
        );

        let mut scoped = changed;
        scoped
            .launch
            .revision
            .content
            .repository_access
            .workflow_change_policy = WorkflowChangePolicy::RequireScopeAndConfirmation;
        scoped
            .launch
            .connection
            .as_mut()
            .unwrap()
            .satisfied_scope_classes
            .push(ScopeClass::Workflow);
        assert_eq!(
            build(
                &scoped,
                &scoped.launch.branch.as_ref().unwrap().current_head,
                10,
            )
            .unwrap()
            .artifact
            .validation,
            DiffValidationOutcome::Valid
        );
    }

    #[test]
    fn changed_file_limit_and_launch_summary_are_bounded() {
        let current = (0..=MAX_DIFF_SUMMARY_FILES)
            .map(|index| regular(&format!("generated/file-{index:04}.txt"), 0o100644, b"x\n"))
            .collect();
        let fixture = fixture(Vec::new(), current);
        let built = build(
            &fixture,
            &fixture.launch.branch.as_ref().unwrap().current_head,
            MAX_DIFF_SUMMARY_FILES as u32,
        )
        .unwrap();
        assert_eq!(
            built.artifact.validation,
            DiffValidationOutcome::TooManyChanges
        );
        assert_eq!(
            built.artifact.total_changed_files as usize,
            MAX_DIFF_SUMMARY_FILES + 1
        );
        assert_eq!(built.artifact.files.len(), MAX_DIFF_SUMMARY_FILES);
        assert!(String::from_utf8(built.object)
            .unwrap()
            .contains("generated/file-1000.txt"));
        let patch_bytes: usize = built
            .artifact
            .files
            .iter()
            .filter_map(|file| file.patch.as_ref())
            .map(String::len)
            .sum();
        assert!(patch_bytes <= MAX_DIFF_SUMMARY_PATCH_BYTES);
    }

    #[test]
    fn missing_corrupt_unsupported_and_oversized_outcomes_are_compact_and_non_publishable() {
        let fixture = fixture(
            vec![regular("README.md", 0o100644, b"old\n")],
            vec![regular("README.md", 0o100644, b"new\n")],
        );
        let checkpoint = fixture
            .launch
            .workspace
            .as_ref()
            .unwrap()
            .latest_checkpoint
            .as_ref()
            .unwrap();
        for outcome in [
            DiffValidationOutcome::MissingBaseline,
            DiffValidationOutcome::CorruptCheckpoint,
            DiffValidationOutcome::UnsupportedFile,
            DiffValidationOutcome::ArtifactTooLarge,
        ] {
            let mut artifact = build_invalid_diff_artifact(
                &fixture.launch,
                checkpoint,
                &fixture.launch.branch.as_ref().unwrap().current_head,
                1_000,
                deterministic_diff_id(&format!("invalid-{outcome:?}")),
                NOW.into(),
                outcome,
            )
            .unwrap()
            .artifact;
            artifact.object_reference = format!("test/{outcome:?}.json");
            assert!(artifact.files.is_empty());
            assert_eq!(artifact.total_changed_files, 0);
            artifact.validate_for_review(&fixture.launch).unwrap();
            assert!(artifact.validate_for_publication(&fixture.launch).is_err());
        }
    }

    #[test]
    fn malformed_archives_headers_offsets_blob_ids_and_payloads_fail_closed() {
        let original = fixture(
            vec![regular("README.md", 0o100644, b"old\n")],
            vec![regular("README.md", 0o100644, b"new\n")],
        );
        let observed = original
            .launch
            .branch
            .as_ref()
            .unwrap()
            .current_head
            .clone();

        let mut truncated = original.checkpoint_archive.clone();
        truncated.truncate(truncated.len() - 1);
        assert_corrupt(with_checkpoint_archive(&original, truncated), &observed);

        let trailing = {
            let mut bytes = original.checkpoint_archive.clone();
            bytes.push(0);
            bytes
        };
        assert_corrupt(with_checkpoint_archive(&original, trailing), &observed);

        let bad_offset = rewrite_header(&original.checkpoint_archive, |header| {
            header["entries"][0]["offset"] = serde_json::json!(1);
        });
        assert_corrupt(with_checkpoint_archive(&original, bad_offset), &observed);

        let unknown_header = rewrite_header(&original.checkpoint_archive, |header| {
            header["unexpected"] = serde_json::json!(true);
        });
        assert_corrupt(
            with_checkpoint_archive(&original, unknown_header),
            &observed,
        );

        let bad_blob = fixture(
            vec![regular("README.md", 0o100644, b"old\n")],
            vec![EntryFixture {
                provider_object_id: Some("f".repeat(40)),
                ..regular("README.md", 0o100644, b"new\n")
            }],
        );
        assert!(matches!(
            build(&bad_blob, &observed, 10),
            Err(DiffBuildError::Corrupt(_))
        ));
    }

    #[test]
    fn unsafe_paths_and_symlinks_are_rejected_before_artifact_generation() {
        for path in ["../secret", "/absolute", "a//b", "a\\b", ".git/config"] {
            let candidate = regular(path, 0o100644, b"value");
            assert!(
                WorkspaceManifest::seal("a".repeat(40), vec![workspace_entry(&candidate)]).is_err()
            );
        }
        let escaping = EntryFixture {
            path: "nested/link".into(),
            kind: WorkspaceEntryKind::Symlink,
            mode: 0o120000,
            bytes: b"../../secret".to_vec(),
            provider_object_id: Some(git_blob_id(b"../../secret")),
            symlink_target: Some("../../secret".into()),
        };
        assert!(WorkspaceManifest::seal("a".repeat(40), vec![workspace_entry(&escaping)]).is_err());
    }

    fn build(
        fixture: &DiffFixture,
        observed_head: &str,
        changed_file_limit: u32,
    ) -> Result<BuiltDiffArtifact, DiffBuildError> {
        let checkpoint = fixture
            .launch
            .workspace
            .as_ref()
            .unwrap()
            .latest_checkpoint
            .as_ref()
            .unwrap();
        build_diff_artifact(
            &fixture.launch,
            checkpoint,
            &fixture.baseline_archive,
            &fixture.checkpoint_archive,
            observed_head,
            changed_file_limit,
            deterministic_diff_id("test-diff-intent"),
            NOW.into(),
        )
    }

    fn fixture(baseline: Vec<EntryFixture>, current: Vec<EntryFixture>) -> DiffFixture {
        let mut launch = crate::trigger::worker::tests::launch_fixture();
        let source_revision = launch.source_sha.clone();
        let baseline_manifest = WorkspaceManifest::seal(
            source_revision.clone(),
            baseline.iter().map(workspace_entry).collect(),
        )
        .unwrap();
        let current_manifest = WorkspaceManifest::seal(
            source_revision.clone(),
            current.iter().map(workspace_entry).collect(),
        )
        .unwrap();
        let baseline_archive = archive(&baseline_manifest, 1, 0, 0, &baseline);
        let checkpoint_archive = archive(&current_manifest, 1, 1, 1, &current);
        let checkpoint = WorkspaceCheckpoint {
            id: CheckpointId::new(),
            launch_id: launch.id.clone(),
            version: 1,
            workspace_epoch: 1,
            session_id: SessionId("sesn_diff_fixture".into()),
            runtime_generation: 1,
            object_reference: "test/checkpoint-v1.pack".into(),
            archive_sha256: hex(&Sha256::digest(&checkpoint_archive)),
            manifest: current_manifest,
            integrity: CheckpointIntegrity::Complete,
            recovery_pair: None,
            created_at: NOW.into(),
            delete_after: Some(DELETE_AFTER.into()),
        };
        launch.branch = Some(BranchRecord {
            name: format!("cma/{}", launch.id.0),
            expected_head: source_revision.clone(),
            current_head: source_revision,
            canonical_url: "https://github.com/contributor/project/tree/cma/test".into(),
            created_at: NOW.into(),
        });
        launch.workspace = Some(WorkspaceRecord {
            baseline: baseline_manifest,
            baseline_object_reference: "test/baseline.pack".into(),
            baseline_archive_sha256: hex(&Sha256::digest(&baseline_archive)),
            latest_checkpoint: Some(checkpoint.clone()),
            previous_checkpoint: None,
            retired_checkpoint: None,
            sealed_contribution_checkpoint: Some(checkpoint),
            checkpoint_intent: None,
            lifecycle_checkpoint: Default::default(),
            checkpoint_version: 1,
            workspace_epoch: 1,
            retention_policy_id: "test-retention-v1".into(),
            delete_after: DELETE_AFTER.into(),
        });
        launch.contribution_checkpoint_version = Some(1);
        DiffFixture {
            launch,
            baseline_archive,
            checkpoint_archive,
        }
    }

    fn archive(
        manifest: &WorkspaceManifest,
        workspace_epoch: u64,
        checkpoint_version: u64,
        quiesced_writers: u32,
        fixtures: &[EntryFixture],
    ) -> Vec<u8> {
        let by_path = fixtures
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        let mut offset = 0_u64;
        let mut payload = Vec::new();
        let entries = manifest
            .entries
            .iter()
            .map(|entry| {
                let fixture = by_path[entry.path.as_str()];
                let wire = ArchiveEntry {
                    path: entry.path.clone(),
                    kind: entry.kind,
                    mode: entry.mode,
                    size: entry.size,
                    content_sha256: entry.content_sha256.clone(),
                    provider_object_id: entry.provider_object_id.clone(),
                    offset,
                };
                offset += entry.size;
                payload.extend_from_slice(&fixture.bytes);
                wire
            })
            .collect();
        let header = ArchiveHeader {
            cma_workspace: WORKSPACE_ARCHIVE_VERSION,
            workspace_epoch,
            checkpoint_version,
            quiesced_writers,
            source_revision: manifest.source_revision.clone(),
            manifest_digest: manifest.digest.clone(),
            entries,
        };
        let header = serde_json::to_vec(&header).unwrap();
        let mut result = Vec::with_capacity(8 + header.len() + payload.len());
        result.extend_from_slice(&(header.len() as u64).to_be_bytes());
        result.extend_from_slice(&header);
        result.extend_from_slice(&payload);
        result
    }

    fn regular(path: &str, mode: u32, bytes: &[u8]) -> EntryFixture {
        EntryFixture {
            path: path.into(),
            kind: WorkspaceEntryKind::Regular,
            mode,
            bytes: bytes.to_vec(),
            provider_object_id: Some(git_blob_id(bytes)),
            symlink_target: None,
        }
    }

    fn symlink(path: &str, target: &str) -> EntryFixture {
        EntryFixture {
            path: path.into(),
            kind: WorkspaceEntryKind::Symlink,
            mode: 0o120000,
            bytes: target.as_bytes().to_vec(),
            provider_object_id: Some(git_blob_id(target.as_bytes())),
            symlink_target: Some(target.into()),
        }
    }

    fn workspace_entry(fixture: &EntryFixture) -> WorkspaceEntry {
        WorkspaceEntry {
            path: fixture.path.clone(),
            kind: fixture.kind,
            mode: fixture.mode,
            size: fixture.bytes.len() as u64,
            content_sha256: hex(&Sha256::digest(&fixture.bytes)),
            provider_object_id: fixture.provider_object_id.clone(),
            symlink_target: fixture.symlink_target.clone(),
        }
    }

    fn with_checkpoint_archive(fixture: &DiffFixture, archive: Vec<u8>) -> DiffFixture {
        let mut launch = fixture.launch.clone();
        launch
            .workspace
            .as_mut()
            .unwrap()
            .latest_checkpoint
            .as_mut()
            .unwrap()
            .archive_sha256 = hex(&Sha256::digest(&archive));
        DiffFixture {
            launch,
            baseline_archive: fixture.baseline_archive.clone(),
            checkpoint_archive: archive,
        }
    }

    fn rewrite_header(archive: &[u8], mutate: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let header_len =
            usize::try_from(u64::from_be_bytes(archive[..8].try_into().unwrap())).unwrap();
        let mut header: serde_json::Value =
            serde_json::from_slice(&archive[8..8 + header_len]).unwrap();
        mutate(&mut header);
        let encoded = serde_json::to_vec(&header).unwrap();
        let mut result = Vec::new();
        result.extend_from_slice(&(encoded.len() as u64).to_be_bytes());
        result.extend_from_slice(&encoded);
        result.extend_from_slice(&archive[8 + header_len..]);
        result
    }

    fn assert_corrupt(fixture: DiffFixture, observed: &str) {
        assert!(matches!(
            build(&fixture, observed, 10),
            Err(DiffBuildError::Corrupt(_))
        ));
    }
}
