//! Byte-faithful repository import through GitHub's tree and blob APIs.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine as _;
use layer2_core::{
    validate_workspace_path, BaselineArchivePlan, BaselineImportBlob, BaselineImportChunk, Launch,
    WorkspaceEntry, WorkspaceEntryKind, WorkspaceManifest, MAX_PROVIDER_BLOB_BYTES,
    MAX_REPOSITORY_BYTES, MAX_REPOSITORY_FILES,
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest as _, Sha256};

use super::{
    BranchContext, BranchReconciliation, GithubBroker, GithubBrokerError, GithubTreeWire, Observed,
    MAX_GITHUB_TREE_ENTRIES,
};

const ARCHIVE_VERSION: u32 = 1;
const MAX_ARCHIVE_HEADER_BYTES: usize = 128 * 1024 * 1024;
const LFS_FILTER: &[u8] = b"filter=lfs";
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceUnsupported {
    TreeIncomplete,
    FileCount,
    FileSize,
    RepositorySize,
    UnsafePath,
    UnsupportedMode,
    Submodule,
    LargeFileStorage,
}

impl std::fmt::Display for WorkspaceUnsupported {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TreeIncomplete => "recursive tree is incomplete",
            Self::FileCount => "file count exceeds the configured limit",
            Self::FileSize => "one Git blob exceeds the configured limit",
            Self::RepositorySize => "repository bytes exceed the configured limit",
            Self::UnsafePath => "a path or symlink target is unsafe",
            Self::UnsupportedMode => "a Git entry mode is unsupported",
            Self::Submodule => "submodules are not supported in the public MVP",
            Self::LargeFileStorage => "Git LFS content is not supported in the public MVP",
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceContext {
    pub branch: BranchContext,
}

impl TryFrom<&Launch> for WorkspaceContext {
    type Error = GithubBrokerError;

    fn try_from(launch: &Launch) -> Result<Self, Self::Error> {
        Ok(Self {
            branch: BranchContext::try_from(launch)?,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceSourceEntry {
    pub path: String,
    pub mode: u32,
    pub provider_object_id: String,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceSourcePlan {
    pub source_revision: String,
    pub tree_digest: String,
    pub entries: Vec<WorkspaceSourceEntry>,
    pub blobs: Vec<BaselineImportBlob>,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceBlobMetadata {
    pub content_sha256: String,
    pub chunks: Vec<BaselineImportChunk>,
    pub lfs_pointer: bool,
    pub contains_lfs_filter: bool,
    pub symlink_target: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubBlobWire {
    sha: String,
    size: u64,
    encoding: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ArchiveHeader<'a> {
    cma_workspace: u32,
    workspace_epoch: u64,
    source_revision: &'a str,
    manifest_digest: &'a str,
    entries: Vec<ArchiveEntry<'a>>,
}

#[derive(Debug, Serialize)]
struct ArchiveEntry<'a> {
    path: &'a str,
    kind: WorkspaceEntryKind,
    mode: u32,
    size: u64,
    content_sha256: &'a str,
    provider_object_id: Option<&'a str>,
    offset: u64,
}

impl GithubBroker {
    /// Import the exact source tree after proving the retained launch ref.
    pub(crate) async fn import_workspace(
        &self,
        context: &WorkspaceContext,
    ) -> Result<WorkspaceSourcePlan, GithubBrokerError> {
        match self.reconcile_branch(&context.branch).await? {
            BranchReconciliation::Ready(_) => {}
            BranchReconciliation::Absent => return Err(GithubBrokerError::AuthorityDrift),
        }
        let repository = self.current_branch_fork(&context.branch).await?;
        let source = &context.branch.source_sha;
        let tree: Observed<GithubTreeWire> = self
            .github_json(
                &context.branch.owner,
                &context.branch.connection.service_id,
                Method::GET,
                &[
                    "repos",
                    repository.canonical_owner.as_str(),
                    repository.canonical_name.as_str(),
                    "git",
                    "trees",
                    source,
                ],
                &[("recursive", "1")],
                None,
            )
            .await?;
        if tree.value.truncated || tree.value.tree.len() > MAX_GITHUB_TREE_ENTRIES {
            return Err(unsupported(WorkspaceUnsupported::TreeIncomplete));
        }

        let mut seen = BTreeSet::new();
        let mut entries = Vec::new();
        let mut blobs = BTreeMap::new();
        let mut declared_bytes = 0_u64;
        for entry in tree.value.tree {
            if validate_workspace_path(&entry.path).is_err() || !seen.insert(entry.path.clone()) {
                return Err(unsupported(WorkspaceUnsupported::UnsafePath));
            }
            match entry.kind.as_str() {
                "tree" => continue,
                "commit" => return Err(unsupported(WorkspaceUnsupported::Submodule)),
                "blob" => {}
                _ => return Err(unsupported(WorkspaceUnsupported::UnsupportedMode)),
            }
            if entries.len() >= MAX_REPOSITORY_FILES {
                return Err(unsupported(WorkspaceUnsupported::FileCount));
            }
            let mode = parse_mode(entry.mode.as_deref())?;
            let object_id = entry.sha.ok_or_else(|| {
                GithubBrokerError::Malformed("a source-tree blob has no object ID".into())
            })?;
            validate_object_id(&object_id)?;
            let size = entry.size.ok_or_else(|| {
                GithubBrokerError::Malformed("a source-tree blob has no declared size".into())
            })?;
            if size > MAX_PROVIDER_BLOB_BYTES {
                return Err(unsupported(WorkspaceUnsupported::FileSize));
            }
            declared_bytes = declared_bytes
                .checked_add(size)
                .ok_or_else(|| unsupported(WorkspaceUnsupported::RepositorySize))?;
            if declared_bytes > MAX_REPOSITORY_BYTES {
                return Err(unsupported(WorkspaceUnsupported::RepositorySize));
            }
            let blob = BaselineImportBlob::new(object_id.clone(), size)
                .map_err(|_| unsupported(WorkspaceUnsupported::FileSize))?;
            if blobs
                .insert(object_id.clone(), blob.clone())
                .is_some_and(|previous| previous != blob)
            {
                return Err(GithubBrokerError::AuthorityDrift);
            }
            entries.push(WorkspaceSourceEntry {
                path: entry.path,
                mode,
                provider_object_id: object_id,
                bytes: size,
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let tree_digest = source_tree_digest(source, &entries);
        Ok(WorkspaceSourcePlan {
            source_revision: source.clone(),
            tree_digest,
            entries,
            blobs: blobs.into_values().collect(),
        })
    }

    pub(crate) async fn read_workspace_blob(
        &self,
        context: &WorkspaceContext,
        expected: &BaselineImportBlob,
    ) -> Result<Vec<u8>, GithubBrokerError> {
        if expected.bytes > MAX_PROVIDER_BLOB_BYTES {
            return Err(unsupported(WorkspaceUnsupported::FileSize));
        }
        let repository = self.current_branch_fork(&context.branch).await?;
        let object_id = &expected.provider_object_id;
        let blob: Observed<GithubBlobWire> = self
            .github_json(
                &context.branch.owner,
                &context.branch.connection.service_id,
                Method::GET,
                &[
                    "repos",
                    repository.canonical_owner.as_str(),
                    repository.canonical_name.as_str(),
                    "git",
                    "blobs",
                    object_id,
                ],
                &[],
                None,
            )
            .await?;
        if !blob.value.sha.eq_ignore_ascii_case(object_id) || blob.value.encoding != "base64" {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        let encoded: String = blob
            .value
            .content
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| GithubBrokerError::Malformed("a Git blob is not valid base64".into()))?;
        if blob.value.size != expected.bytes
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX) != blob.value.size
            || git_blob_id(&bytes) != object_id.to_ascii_lowercase()
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        Ok(bytes)
    }
}

pub(crate) fn workspace_blob_metadata(
    blob: &BaselineImportBlob,
    bytes: &[u8],
    chunks: Vec<BaselineImportChunk>,
    needs_symlink_target: bool,
) -> Result<WorkspaceBlobMetadata, GithubBrokerError> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != blob.bytes
        || git_blob_id(bytes) != blob.provider_object_id
    {
        return Err(GithubBrokerError::AuthorityDrift);
    }
    let chunk_bytes = chunks.iter().try_fold(0_u64, |total, chunk| {
        total
            .checked_add(chunk.bytes)
            .ok_or_else(|| unsupported(WorkspaceUnsupported::RepositorySize))
    })?;
    if chunk_bytes != blob.bytes || chunks.len() != blob.chunk_count as usize {
        return Err(GithubBrokerError::AuthorityDrift);
    }
    let symlink_target = needs_symlink_target
        .then(|| {
            std::str::from_utf8(bytes)
                .map(str::to_string)
                .map_err(|_| unsupported(WorkspaceUnsupported::UnsafePath))
        })
        .transpose()?;
    Ok(WorkspaceBlobMetadata {
        content_sha256: hex(&Sha256::digest(bytes)),
        chunks,
        lfs_pointer: bytes.starts_with(LFS_POINTER),
        contains_lfs_filter: bytes
            .windows(LFS_FILTER.len())
            .any(|window| window.eq_ignore_ascii_case(LFS_FILTER)),
        symlink_target,
    })
}

pub(crate) fn build_baseline_archive_plan(
    source: &WorkspaceSourcePlan,
    blobs: &BTreeMap<String, WorkspaceBlobMetadata>,
) -> Result<(WorkspaceManifest, BaselineArchivePlan), GithubBrokerError> {
    let mut entries = Vec::with_capacity(source.entries.len());
    let mut ordered_chunks = Vec::new();
    for entry in &source.entries {
        let blob = blobs
            .get(&entry.provider_object_id)
            .ok_or(GithubBrokerError::AuthorityDrift)?;
        if blob.lfs_pointer || (entry.path.ends_with(".gitattributes") && blob.contains_lfs_filter)
        {
            return Err(unsupported(WorkspaceUnsupported::LargeFileStorage));
        }
        let kind = match entry.mode {
            0o100644 | 0o100755 => WorkspaceEntryKind::Regular,
            0o120000 => WorkspaceEntryKind::Symlink,
            _ => return Err(unsupported(WorkspaceUnsupported::UnsupportedMode)),
        };
        let manifest_entry = WorkspaceEntry {
            path: entry.path.clone(),
            kind,
            mode: entry.mode,
            size: entry.bytes,
            content_sha256: blob.content_sha256.clone(),
            provider_object_id: Some(entry.provider_object_id.clone()),
            symlink_target: (kind == WorkspaceEntryKind::Symlink)
                .then(|| blob.symlink_target.clone())
                .flatten(),
        };
        manifest_entry
            .validate()
            .map_err(|_| unsupported(WorkspaceUnsupported::UnsafePath))?;
        entries.push(manifest_entry);
        ordered_chunks.extend(blob.chunks.iter().cloned());
    }
    let manifest = WorkspaceManifest::seal(source.source_revision.clone(), entries)
        .map_err(|_| unsupported(WorkspaceUnsupported::RepositorySize))?;

    let mut offset = 0_u64;
    let mut header_entries = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        header_entries.push(ArchiveEntry {
            path: &entry.path,
            kind: entry.kind,
            mode: entry.mode,
            size: entry.size,
            content_sha256: &entry.content_sha256,
            provider_object_id: entry.provider_object_id.as_deref(),
            offset,
        });
        offset = offset
            .checked_add(entry.size)
            .ok_or_else(|| unsupported(WorkspaceUnsupported::RepositorySize))?;
    }
    let header = serde_json::to_vec(&ArchiveHeader {
        cma_workspace: ARCHIVE_VERSION,
        workspace_epoch: 1,
        source_revision: &manifest.source_revision,
        manifest_digest: &manifest.digest,
        entries: header_entries,
    })
    .map_err(|_| GithubBrokerError::Malformed("workspace header cannot be encoded".into()))?;
    if header.len() > MAX_ARCHIVE_HEADER_BYTES {
        return Err(unsupported(WorkspaceUnsupported::FileCount));
    }
    let header_len = u64::try_from(header.len())
        .map_err(|_| unsupported(WorkspaceUnsupported::RepositorySize))?;
    let mut preamble = Vec::with_capacity(8_usize.saturating_add(header.len()));
    preamble.extend_from_slice(&header_len.to_be_bytes());
    preamble.extend_from_slice(&header);
    let archive_bytes = u64::try_from(preamble.len())
        .unwrap_or(u64::MAX)
        .checked_add(offset)
        .ok_or_else(|| unsupported(WorkspaceUnsupported::RepositorySize))?;
    Ok((
        manifest,
        BaselineArchivePlan {
            source_revision: source.source_revision.clone(),
            tree_digest: source.tree_digest.clone(),
            preamble,
            chunks: ordered_chunks,
            archive_bytes,
        },
    ))
}

fn source_tree_digest(source: &str, entries: &[WorkspaceSourceEntry]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cma-trigger-source-tree-v1\0");
    digest.update(source.as_bytes());
    for entry in entries {
        digest.update((entry.path.len() as u64).to_be_bytes());
        digest.update(entry.path.as_bytes());
        digest.update(entry.mode.to_be_bytes());
        digest.update(entry.provider_object_id.as_bytes());
        digest.update(entry.bytes.to_be_bytes());
    }
    hex(&digest.finalize())
}

fn parse_mode(mode: Option<&str>) -> Result<u32, GithubBrokerError> {
    match mode {
        Some("100644") => Ok(0o100644),
        Some("100755") => Ok(0o100755),
        Some("120000") => Ok(0o120000),
        Some("160000") => Err(unsupported(WorkspaceUnsupported::Submodule)),
        _ => Err(unsupported(WorkspaceUnsupported::UnsupportedMode)),
    }
}

fn validate_object_id(value: &str) -> Result<(), GithubBrokerError> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GithubBrokerError::Malformed(
            "a Git blob object ID is invalid".into(),
        ));
    }
    Ok(())
}

fn git_blob_id(bytes: &[u8]) -> String {
    let mut digest = Sha1::new();
    digest.update(format!("blob {}\0", bytes.len()).as_bytes());
    digest.update(bytes);
    hex(&digest.finalize())
}

fn is_lfs_content(path: &str, bytes: &[u8]) -> bool {
    bytes.starts_with(LFS_POINTER)
        || (path.ends_with(".gitattributes")
            && bytes
                .windows(LFS_FILTER.len())
                .any(|window| window.eq_ignore_ascii_case(LFS_FILTER)))
}

fn unsupported(reason: WorkspaceUnsupported) -> GithubBrokerError {
    GithubBrokerError::UnsupportedWorkspace(reason)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_blob_identity_includes_the_git_object_header() {
        assert_eq!(
            git_blob_id(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
    }

    #[test]
    fn lfs_attributes_and_pointer_files_are_both_refused() {
        assert!(is_lfs_content(
            ".gitattributes",
            b"*.bin filter=lfs diff=lfs merge=lfs -text\n"
        ));
        assert!(is_lfs_content(
            "large.bin",
            b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\n"
        ));
        assert!(!is_lfs_content("README.md", b"ordinary bytes"));
    }
}
