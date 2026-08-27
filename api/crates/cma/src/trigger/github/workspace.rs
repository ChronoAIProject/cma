//! Byte-faithful repository import through GitHub's tree and blob APIs.

use std::collections::BTreeSet;

use base64::Engine as _;
use layer2_core::{
    validate_workspace_path, Launch, WorkspaceEntry, WorkspaceEntryKind, WorkspaceManifest,
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
pub(crate) struct ImportedWorkspace {
    pub manifest: WorkspaceManifest,
    pub archive: Vec<u8>,
    pub archive_sha256: String,
}

#[derive(Debug, Deserialize)]
struct GithubBlobWire {
    sha: String,
    size: u64,
    encoding: String,
    content: String,
}

#[derive(Debug)]
struct ImportedEntry {
    manifest: WorkspaceEntry,
    bytes: Vec<u8>,
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
    ) -> Result<ImportedWorkspace, GithubBrokerError> {
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
        let mut blobs = Vec::new();
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
            if blobs.len() >= MAX_REPOSITORY_FILES {
                return Err(unsupported(WorkspaceUnsupported::FileCount));
            }
            let mode = parse_mode(entry.mode.as_deref())?;
            let object_id = entry.sha.ok_or_else(|| {
                GithubBrokerError::Malformed("a source-tree blob has no object ID".into())
            })?;
            validate_object_id(&object_id)?;
            if let Some(size) = entry.size {
                declared_bytes = declared_bytes
                    .checked_add(size)
                    .ok_or_else(|| unsupported(WorkspaceUnsupported::RepositorySize))?;
                if declared_bytes > MAX_REPOSITORY_BYTES {
                    return Err(unsupported(WorkspaceUnsupported::RepositorySize));
                }
            }
            blobs.push((entry.path, mode, object_id));
        }

        let mut imported = Vec::with_capacity(blobs.len());
        let mut total = 0_u64;
        for (path, mode, object_id) in blobs {
            let bytes = self.read_blob(context, &repository, &object_id).await?;
            total = total
                .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
                .ok_or_else(|| unsupported(WorkspaceUnsupported::RepositorySize))?;
            if total > MAX_REPOSITORY_BYTES {
                return Err(unsupported(WorkspaceUnsupported::RepositorySize));
            }
            if is_lfs_content(&path, &bytes) {
                return Err(unsupported(WorkspaceUnsupported::LargeFileStorage));
            }
            let kind = match mode {
                0o100644 | 0o100755 => WorkspaceEntryKind::Regular,
                0o120000 => WorkspaceEntryKind::Symlink,
                _ => return Err(unsupported(WorkspaceUnsupported::UnsupportedMode)),
            };
            let symlink_target = if kind == WorkspaceEntryKind::Symlink {
                Some(
                    std::str::from_utf8(&bytes)
                        .map_err(|_| unsupported(WorkspaceUnsupported::UnsafePath))?
                        .to_string(),
                )
            } else {
                None
            };
            let manifest = WorkspaceEntry {
                path,
                kind,
                mode,
                size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                content_sha256: hex(&Sha256::digest(&bytes)),
                provider_object_id: Some(object_id),
                symlink_target,
            };
            manifest
                .validate()
                .map_err(|_| unsupported(WorkspaceUnsupported::UnsafePath))?;
            imported.push(ImportedEntry { manifest, bytes });
        }
        imported.sort_by(|left, right| left.manifest.path.cmp(&right.manifest.path));
        let manifest = WorkspaceManifest::seal(
            source.clone(),
            imported
                .iter()
                .map(|entry| entry.manifest.clone())
                .collect(),
        )
        .map_err(|_| unsupported(WorkspaceUnsupported::RepositorySize))?;
        let archive = seal_archive(&manifest, &imported)?;
        let archive_sha256 = hex(&Sha256::digest(&archive));
        Ok(ImportedWorkspace {
            manifest,
            archive,
            archive_sha256,
        })
    }

    async fn read_blob(
        &self,
        context: &WorkspaceContext,
        repository: &layer2_core::RepositoryIdentity,
        object_id: &str,
    ) -> Result<Vec<u8>, GithubBrokerError> {
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
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != blob.value.size
            || git_blob_id(&bytes) != object_id.to_ascii_lowercase()
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        Ok(bytes)
    }
}

fn seal_archive(
    manifest: &WorkspaceManifest,
    entries: &[ImportedEntry],
) -> Result<Vec<u8>, GithubBrokerError> {
    let mut offset = 0_u64;
    let mut header_entries = Vec::with_capacity(entries.len());
    for entry in entries {
        header_entries.push(ArchiveEntry {
            path: &entry.manifest.path,
            kind: entry.manifest.kind,
            mode: entry.manifest.mode,
            size: entry.manifest.size,
            content_sha256: &entry.manifest.content_sha256,
            provider_object_id: entry.manifest.provider_object_id.as_deref(),
            offset,
        });
        offset = offset
            .checked_add(entry.manifest.size)
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
    let payload_len =
        usize::try_from(offset).map_err(|_| unsupported(WorkspaceUnsupported::RepositorySize))?;
    let mut archive = Vec::with_capacity(
        8_usize
            .saturating_add(header.len())
            .saturating_add(payload_len),
    );
    archive.extend_from_slice(&header_len.to_be_bytes());
    archive.extend_from_slice(&header);
    for entry in entries {
        archive.extend_from_slice(&entry.bytes);
    }
    Ok(archive)
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
