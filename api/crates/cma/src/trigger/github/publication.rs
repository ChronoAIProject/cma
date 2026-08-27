//! Guarded Git Data publication for one explicitly confirmed Trigger diff.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine as _;
use layer2_core::{
    ConnectionSnapshot, ContributionRecord, DiffChangeKind, ForkRecord, Launch, PublicationState,
    SubjectId, MAX_DIFF_BYTES,
};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

use super::{
    repository_ref_components, validate_commit_sha, BranchContext, GithubBroker, GithubBrokerError,
    GithubRejectionKind, Observed, WorkspaceUnsupported, MAX_GITHUB_TREE_ENTRIES,
};
use crate::trigger::workspace_diff::{git_blob_id, is_binary, PublicationFile, PublicationPlan};

const MAX_INLINE_FILE_BYTES: usize = 128 * 1024;
const MAX_INLINE_TREE_BYTES: usize = 4 * 1024 * 1024;
const LFS_FILTER: &[u8] = b"filter=lfs";
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1";

#[derive(Debug, Clone)]
pub(crate) struct PublicationContext {
    owner: SubjectId,
    connection: ConnectionSnapshot,
    fork: ForkRecord,
    launch_id: String,
    branch_name: String,
    contribution: ContributionRecord,
}

impl TryFrom<&Launch> for PublicationContext {
    type Error = GithubBrokerError;

    fn try_from(launch: &Launch) -> Result<Self, Self::Error> {
        let contribution = launch.contribution.clone().ok_or_else(|| {
            GithubBrokerError::Malformed("the Launch has no publication intent".into())
        })?;
        contribution
            .validate_intent(&launch.id)
            .map_err(|error| GithubBrokerError::Malformed(error.to_string()))?;
        if !matches!(
            contribution.state,
            PublicationState::IntentRecorded
                | PublicationState::ObjectsWritten
                | PublicationState::RefUpdatePending
        ) {
            return Err(GithubBrokerError::Malformed(
                "the publication intent is not writable".into(),
            ));
        }
        let branch = launch.branch.as_ref().ok_or_else(|| {
            GithubBrokerError::Malformed("the Launch has no guarded branch".into())
        })?;
        if contribution.branch_name != branch.name
            || contribution.expected_head != branch.current_head
            || branch.name != format!("cma/{}", launch.id.0)
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        Ok(Self {
            owner: launch.owner.clone(),
            connection: launch.connection.clone().ok_or_else(|| {
                GithubBrokerError::Malformed("the Launch has no selected connection".into())
            })?,
            fork: launch.fork.clone().ok_or_else(|| {
                GithubBrokerError::Malformed("the Launch has no reconciled fork".into())
            })?,
            launch_id: launch.id.0.clone(),
            branch_name: branch.name.clone(),
            contribution,
        })
    }
}

#[cfg(test)]
impl PublicationContext {
    pub(super) fn fixture(branch: &BranchContext, contribution: ContributionRecord) -> Self {
        Self {
            owner: branch.owner.clone(),
            connection: branch.connection.clone(),
            fork: branch.fork.clone(),
            launch_id: branch.launch_id.clone(),
            branch_name: format!("cma/{}", branch.launch_id),
            contribution,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublishedGitData {
    pub(crate) base_tree_id: String,
    pub(crate) tree_id: String,
    pub(crate) commit_id: String,
    pub(crate) resulting_head: String,
    pub(crate) object_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedGitData {
    pub(crate) base_tree_id: String,
    pub(crate) tree_id: String,
    pub(crate) commit_id: String,
    pub(crate) object_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PublicationAdvance {
    ObjectsWritten(PreparedGitData),
    RefUpdatePending(PreparedGitData),
    Published(PublishedGitData),
}

#[derive(Debug, Deserialize)]
struct BlobWire {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct TreeWriteWire {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct CommitWire {
    sha: String,
    tree: ObjectWire,
    #[serde(default)]
    parents: Vec<ObjectWire>,
}

#[derive(Debug, Deserialize)]
struct ObjectWire {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct RefWire {
    #[serde(rename = "ref")]
    reference: String,
    object: RefObjectWire,
}

#[derive(Debug, Deserialize)]
struct RefObjectWire {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct TreeReadWire {
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    tree: Vec<TreeEntryWire>,
}

#[derive(Debug, Deserialize)]
struct TreeEntryWire {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
}

impl GithubBroker {
    pub(crate) async fn publish_branch(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
    ) -> Result<PublicationAdvance, GithubBrokerError> {
        validate_plan(context, plan)?;
        reject_lfs(plan)?;
        let branch_context = BranchContext {
            owner: context.owner.clone(),
            launch_id: context.launch_id.clone(),
            connection: context.connection.clone(),
            fork: context.fork.clone(),
            source_sha: plan.expected_head.clone(),
        };
        let repository = self.current_branch_fork(&branch_context).await?;
        match context.contribution.state {
            PublicationState::IntentRecorded => self
                .write_publication_objects(context, plan, &repository)
                .await
                .map(PublicationAdvance::ObjectsWritten),
            PublicationState::ObjectsWritten => {
                let prepared = retained_prepared(context)?;
                let observed_head = self
                    .resolve_branch_sha(
                        &context.owner,
                        &context.connection.service_id,
                        &repository,
                        &context.branch_name,
                    )
                    .await?;
                if observed_head.eq_ignore_ascii_case(&prepared.commit_id) {
                    return self
                        .verified_result(
                            context,
                            plan,
                            &repository,
                            prepared.base_tree_id,
                            prepared.tree_id,
                            prepared.commit_id,
                            prepared.object_ids,
                        )
                        .await
                        .map(PublicationAdvance::Published);
                }
                if !observed_head.eq_ignore_ascii_case(&plan.expected_head) {
                    return Err(GithubBrokerError::BranchHeadConflict);
                }
                let commit = self
                    .read_commit(context, &repository, &prepared.commit_id)
                    .await?;
                validate_commit_response(&commit, &prepared.tree_id, &plan.expected_head)?;
                Ok(PublicationAdvance::RefUpdatePending(prepared))
            }
            PublicationState::RefUpdatePending => self
                .advance_guarded_ref(context, plan, &repository, retained_prepared(context)?)
                .await
                .map(PublicationAdvance::Published),
            _ => Err(GithubBrokerError::Malformed(
                "the publication state cannot perform a Git Data write".into(),
            )),
        }
    }

    async fn write_publication_objects(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        repository: &layer2_core::RepositoryIdentity,
    ) -> Result<PreparedGitData, GithubBrokerError> {
        let observed_head = self
            .resolve_branch_sha(
                &context.owner,
                &context.connection.service_id,
                repository,
                &context.branch_name,
            )
            .await?;
        if !observed_head.eq_ignore_ascii_case(&plan.expected_head) {
            return Err(GithubBrokerError::BranchHeadConflict);
        }

        let base_commit = self
            .read_commit(context, repository, &plan.expected_head)
            .await?;
        if !base_commit.sha.eq_ignore_ascii_case(&plan.expected_head) {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        validate_commit_sha(&base_commit.tree.sha)?;
        let base_tree_id = base_commit.tree.sha.to_ascii_lowercase();
        let (tree_entries, object_ids) =
            self.prepare_tree_entries(context, repository, plan).await?;
        let tree: Observed<TreeWriteWire> = self
            .github_json(
                &context.owner,
                &context.connection.service_id,
                Method::POST,
                &[
                    "repos",
                    repository.canonical_owner.as_str(),
                    repository.canonical_name.as_str(),
                    "git",
                    "trees",
                ],
                &[],
                Some(json!({
                    "base_tree": base_tree_id,
                    "tree": tree_entries,
                })),
            )
            .await?;
        validate_commit_sha(&tree.value.sha)?;
        let tree_id = tree.value.sha.to_ascii_lowercase();

        let message = format!(
            "CMA Trigger contribution\n\nLaunch: {}\nReviewed diff: {}",
            context.launch_id, plan.diff_digest
        );
        let commit: Observed<CommitWire> = self
            .github_json(
                &context.owner,
                &context.connection.service_id,
                Method::POST,
                &[
                    "repos",
                    repository.canonical_owner.as_str(),
                    repository.canonical_name.as_str(),
                    "git",
                    "commits",
                ],
                &[],
                Some(json!({
                    "message": message,
                    "tree": tree_id,
                    "parents": [plan.expected_head],
                    "author": {
                        "name": plan.author.name,
                        "email": plan.author.email,
                        "date": context.contribution.confirmation.confirmed_at,
                    },
                    "committer": {
                        "name": plan.committer.name,
                        "email": plan.committer.email,
                        "date": context.contribution.confirmation.confirmed_at,
                    },
                })),
            )
            .await?;
        validate_commit_response(&commit.value, &tree_id, &plan.expected_head)?;
        let commit_id = commit.value.sha.to_ascii_lowercase();

        Ok(PreparedGitData {
            base_tree_id,
            tree_id,
            commit_id,
            object_ids,
        })
    }

    async fn advance_guarded_ref(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        repository: &layer2_core::RepositoryIdentity,
        prepared: PreparedGitData,
    ) -> Result<PublishedGitData, GithubBrokerError> {
        let PreparedGitData {
            base_tree_id,
            tree_id,
            commit_id,
            object_ids,
        } = prepared;

        let before_patch = self
            .resolve_branch_sha(
                &context.owner,
                &context.connection.service_id,
                repository,
                &context.branch_name,
            )
            .await?;
        if before_patch.eq_ignore_ascii_case(&commit_id) {
            return self
                .verified_result(
                    context,
                    plan,
                    repository,
                    base_tree_id,
                    tree_id,
                    commit_id,
                    object_ids,
                )
                .await;
        }
        if !before_patch.eq_ignore_ascii_case(&plan.expected_head) {
            return Err(GithubBrokerError::BranchHeadConflict);
        }

        // Last authority read before the only history-changing operation.
        let current = self
            .preflight_connection(&context.owner, &context.connection.service_id)
            .await?;
        if !same_connection(&context.connection, &current.snapshot) {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        let components = repository_ref_components(&context.branch_name)?;
        let mut ref_path = vec![
            "repos",
            repository.canonical_owner.as_str(),
            repository.canonical_name.as_str(),
            "git",
            "refs",
            "heads",
        ];
        ref_path.extend(components);
        let patch: Result<Observed<RefWire>, GithubBrokerError> = self
            .github_json(
                &context.owner,
                &context.connection.service_id,
                Method::PATCH,
                &ref_path,
                &[],
                Some(json!({ "sha": commit_id, "force": false })),
            )
            .await;
        match patch {
            Ok(response) => validate_ref(&response.value, &context.branch_name, &commit_id)?,
            Err(error) => {
                return self
                    .reconcile_patch_error(
                        context,
                        plan,
                        repository,
                        base_tree_id,
                        tree_id,
                        commit_id,
                        object_ids,
                        error,
                    )
                    .await
            }
        }
        self.verified_result(
            context,
            plan,
            repository,
            base_tree_id,
            tree_id,
            commit_id,
            object_ids,
        )
        .await
    }

    async fn prepare_tree_entries(
        &self,
        context: &PublicationContext,
        repository: &layer2_core::RepositoryIdentity,
        plan: &PublicationPlan,
    ) -> Result<(Vec<Value>, Vec<String>), GithubBrokerError> {
        let mut inline_bytes = 0_usize;
        let mut entries = Vec::with_capacity(plan.files.len());
        let mut object_ids = BTreeSet::new();
        for file in &plan.files {
            let entry = if file.change == DiffChangeKind::Deleted {
                json!({
                    "path": file.path,
                    "mode": file.mode,
                    "type": "blob",
                    "sha": Value::Null,
                })
            } else {
                let content = file.content.as_deref().ok_or_else(|| {
                    GithubBrokerError::Malformed("a changed path has no checkpoint bytes".into())
                })?;
                let blob_id = file.blob_id.as_deref().ok_or_else(|| {
                    GithubBrokerError::Malformed("a changed path has no Git blob identity".into())
                })?;
                validate_commit_sha(blob_id)?;
                object_ids.insert(blob_id.to_ascii_lowercase());
                let inline = file.change != DiffChangeKind::ModeChanged
                    && !file.binary
                    && content.len() <= MAX_INLINE_FILE_BYTES
                    && inline_bytes.saturating_add(content.len()) <= MAX_INLINE_TREE_BYTES;
                if inline {
                    let text = std::str::from_utf8(content).map_err(|_| {
                        GithubBrokerError::Malformed("a text path is not valid UTF-8".into())
                    })?;
                    inline_bytes += content.len();
                    json!({
                        "path": file.path,
                        "mode": file.mode,
                        "type": "blob",
                        "content": text,
                    })
                } else if file.change == DiffChangeKind::ModeChanged {
                    json!({
                        "path": file.path,
                        "mode": file.mode,
                        "type": "blob",
                        "sha": blob_id,
                    })
                } else {
                    let response: Observed<BlobWire> = self
                        .github_json(
                            &context.owner,
                            &context.connection.service_id,
                            Method::POST,
                            &[
                                "repos",
                                repository.canonical_owner.as_str(),
                                repository.canonical_name.as_str(),
                                "git",
                                "blobs",
                            ],
                            &[],
                            Some(json!({
                                "content": base64::engine::general_purpose::STANDARD.encode(content),
                                "encoding": "base64",
                            })),
                        )
                        .await?;
                    if !response.value.sha.eq_ignore_ascii_case(blob_id) {
                        return Err(GithubBrokerError::AuthorityDrift);
                    }
                    json!({
                        "path": file.path,
                        "mode": file.mode,
                        "type": "blob",
                        "sha": blob_id,
                    })
                }
            };
            entries.push(entry);
        }
        Ok((entries, object_ids.into_iter().collect()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn reconcile_patch_error(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        repository: &layer2_core::RepositoryIdentity,
        base_tree_id: String,
        tree_id: String,
        commit_id: String,
        object_ids: Vec<String>,
        error: GithubBrokerError,
    ) -> Result<PublishedGitData, GithubBrokerError> {
        let observed = self
            .resolve_branch_sha(
                &context.owner,
                &context.connection.service_id,
                repository,
                &context.branch_name,
            )
            .await;
        match observed {
            Ok(head) if head.eq_ignore_ascii_case(&commit_id) => {
                self.verified_result(
                    context,
                    plan,
                    repository,
                    base_tree_id,
                    tree_id,
                    commit_id,
                    object_ids,
                )
                .await
            }
            Ok(head) if !head.eq_ignore_ascii_case(&plan.expected_head) => {
                Err(GithubBrokerError::BranchHeadConflict)
            }
            Ok(_) => Err(error),
            Err(_) => Err(match error {
                GithubBrokerError::Rejected {
                    status,
                    kind: GithubRejectionKind::NonFastForward,
                    provider_code,
                } => GithubBrokerError::Rejected {
                    status,
                    kind: GithubRejectionKind::NonFastForward,
                    provider_code,
                },
                _ => GithubBrokerError::AmbiguousWrite,
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn verified_result(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        repository: &layer2_core::RepositoryIdentity,
        base_tree_id: String,
        tree_id: String,
        commit_id: String,
        object_ids: Vec<String>,
    ) -> Result<PublishedGitData, GithubBrokerError> {
        self.verify_publication(
            context,
            plan,
            repository,
            &base_tree_id,
            &tree_id,
            &commit_id,
        )
        .await?;
        Ok(PublishedGitData {
            base_tree_id,
            tree_id,
            commit_id: commit_id.clone(),
            resulting_head: commit_id,
            object_ids,
        })
    }

    async fn verify_publication(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        repository: &layer2_core::RepositoryIdentity,
        base_tree_id: &str,
        tree_id: &str,
        commit_id: &str,
    ) -> Result<(), GithubBrokerError> {
        let head = self
            .resolve_branch_sha(
                &context.owner,
                &context.connection.service_id,
                repository,
                &context.branch_name,
            )
            .await?;
        if !head.eq_ignore_ascii_case(commit_id) {
            return Err(GithubBrokerError::AmbiguousWrite);
        }
        let base_commit = self
            .read_commit(context, repository, &plan.expected_head)
            .await?;
        if !base_commit.sha.eq_ignore_ascii_case(&plan.expected_head)
            || !base_commit.tree.sha.eq_ignore_ascii_case(base_tree_id)
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        let commit = self.read_commit(context, repository, commit_id).await?;
        validate_commit_response(&commit, tree_id, &plan.expected_head)?;
        let tree: Observed<TreeReadWire> = self
            .github_json(
                &context.owner,
                &context.connection.service_id,
                Method::GET,
                &[
                    "repos",
                    repository.canonical_owner.as_str(),
                    repository.canonical_name.as_str(),
                    "git",
                    "trees",
                    tree_id,
                ],
                &[("recursive", "1")],
                None,
            )
            .await?;
        if tree.value.truncated || tree.value.tree.len() > MAX_GITHUB_TREE_ENTRIES {
            return Err(GithubBrokerError::Malformed(
                "the resulting publication tree is truncated".into(),
            ));
        }
        let entries = tree
            .value
            .tree
            .into_iter()
            .map(|entry| (entry.path.clone(), entry))
            .collect::<BTreeMap<_, _>>();
        for file in &plan.files {
            match file.content.as_ref() {
                None if entries.contains_key(&file.path) => {
                    return Err(GithubBrokerError::AuthorityDrift)
                }
                None => {}
                Some(_) => {
                    let entry = entries
                        .get(&file.path)
                        .ok_or(GithubBrokerError::AuthorityDrift)?;
                    if entry.kind != "blob"
                        || entry.mode != file.mode
                        || file
                            .blob_id
                            .as_ref()
                            .is_none_or(|blob| !entry.sha.eq_ignore_ascii_case(blob))
                    {
                        return Err(GithubBrokerError::AuthorityDrift);
                    }
                }
            }
        }
        Ok(())
    }

    async fn read_commit(
        &self,
        context: &PublicationContext,
        repository: &layer2_core::RepositoryIdentity,
        commit_id: &str,
    ) -> Result<CommitWire, GithubBrokerError> {
        validate_commit_sha(commit_id)?;
        let commit: Observed<CommitWire> = self
            .github_json(
                &context.owner,
                &context.connection.service_id,
                Method::GET,
                &[
                    "repos",
                    repository.canonical_owner.as_str(),
                    repository.canonical_name.as_str(),
                    "git",
                    "commits",
                    commit_id,
                ],
                &[],
                None,
            )
            .await?;
        Ok(commit.value)
    }
}

fn validate_plan(
    context: &PublicationContext,
    plan: &PublicationPlan,
) -> Result<(), GithubBrokerError> {
    if plan.launch_id.0 != context.launch_id
        || plan.artifact_id != context.contribution.diff_artifact_id
        || plan.revision_digest != context.contribution.revision_digest
        || plan.baseline_digest != context.contribution.baseline_digest
        || plan.checkpoint_digest != context.contribution.checkpoint_digest
        || plan.diff_digest != context.contribution.diff_digest
        || plan.expected_head != context.contribution.expected_head
        || plan.author != context.contribution.author
        || plan.committer != context.contribution.committer
        || plan.files.is_empty()
        || plan.files.len() > layer2_core::MAX_DIFF_SUMMARY_FILES
    {
        return Err(GithubBrokerError::AuthorityDrift);
    }
    validate_commit_sha(&plan.expected_head)?;
    let mut prior_path: Option<&str> = None;
    let mut publication_bytes = 0_u64;
    for file in &plan.files {
        validate_publication_file(file)?;
        if prior_path.is_some_and(|prior| prior >= file.path.as_str()) {
            return Err(GithubBrokerError::Malformed(
                "publication paths are not a sorted unique set".into(),
            ));
        }
        prior_path = Some(file.path.as_str());
        publication_bytes = publication_bytes
            .checked_add(file.content.as_ref().map_or(0, |content| {
                u64::try_from(content.len()).unwrap_or(u64::MAX)
            }))
            .ok_or_else(|| {
                GithubBrokerError::Malformed("publication byte count overflowed".into())
            })?;
    }
    if publication_bytes > MAX_DIFF_BYTES {
        return Err(GithubBrokerError::Malformed(
            "publication bytes exceed the closed write-boundary limit".into(),
        ));
    }
    Ok(())
}

fn validate_publication_file(file: &PublicationFile) -> Result<(), GithubBrokerError> {
    layer2_core::validate_workspace_path(&file.path)
        .map_err(|error| GithubBrokerError::Malformed(error.to_string()))?;
    if !matches!(file.mode.as_str(), "100644" | "100755" | "120000") {
        return Err(GithubBrokerError::Malformed(
            "publication path uses an unsupported Git mode".into(),
        ));
    }
    if file.change == DiffChangeKind::Deleted {
        if file.content.is_some() || file.content_sha256.is_some() || file.blob_id.is_some() {
            return Err(GithubBrokerError::Malformed(
                "a deletion cannot carry replacement blob data".into(),
            ));
        }
        return Ok(());
    }
    let content = file.content.as_deref().ok_or_else(|| {
        GithubBrokerError::Malformed("a changed path has no checkpoint bytes".into())
    })?;
    if file.binary != is_binary(content) {
        return Err(GithubBrokerError::Malformed(
            "a changed path disagrees with its reviewed binary classification".into(),
        ));
    }
    let content_sha256 = file.content_sha256.as_deref().ok_or_else(|| {
        GithubBrokerError::Malformed("a changed path has no checkpoint digest".into())
    })?;
    if content_sha256.len() != 64
        || !content_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !format!("{:x}", Sha256::digest(content)).eq_ignore_ascii_case(content_sha256)
    {
        return Err(GithubBrokerError::Malformed(
            "a changed path failed its checkpoint SHA-256 boundary".into(),
        ));
    }
    let blob_id = file.blob_id.as_deref().ok_or_else(|| {
        GithubBrokerError::Malformed("a changed path has no Git blob identity".into())
    })?;
    validate_commit_sha(blob_id)?;
    if !git_blob_id(content).eq_ignore_ascii_case(blob_id) {
        return Err(GithubBrokerError::Malformed(
            "a changed path failed its Git blob identity boundary".into(),
        ));
    }
    Ok(())
}

fn retained_prepared(context: &PublicationContext) -> Result<PreparedGitData, GithubBrokerError> {
    let prepared = PreparedGitData {
        base_tree_id: context.contribution.base_tree_id.clone().ok_or_else(|| {
            GithubBrokerError::Malformed("the retained publication has no base tree".into())
        })?,
        tree_id: context.contribution.tree_id.clone().ok_or_else(|| {
            GithubBrokerError::Malformed("the retained publication has no resulting tree".into())
        })?,
        commit_id: context.contribution.commit_id.clone().ok_or_else(|| {
            GithubBrokerError::Malformed("the retained publication has no commit".into())
        })?,
        object_ids: context.contribution.object_ids.clone(),
    };
    validate_commit_sha(&prepared.base_tree_id)?;
    validate_commit_sha(&prepared.tree_id)?;
    validate_commit_sha(&prepared.commit_id)?;
    for object_id in &prepared.object_ids {
        validate_commit_sha(object_id)?;
    }
    Ok(prepared)
}

fn reject_lfs(plan: &PublicationPlan) -> Result<(), GithubBrokerError> {
    if plan.files.iter().any(|file| {
        file.content.as_ref().is_some_and(|content| {
            (file.path.ends_with(".gitattributes")
                && contains_ascii_case_insensitive(content, LFS_FILTER))
                || content.starts_with(LFS_POINTER)
        })
    }) {
        return Err(GithubBrokerError::UnsupportedWorkspace(
            WorkspaceUnsupported::LargeFileStorage,
        ));
    }
    Ok(())
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|candidate| {
        candidate
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

fn validate_commit_response(
    commit: &CommitWire,
    tree_id: &str,
    parent_id: &str,
) -> Result<(), GithubBrokerError> {
    validate_commit_sha(&commit.sha)?;
    if !commit.tree.sha.eq_ignore_ascii_case(tree_id)
        || commit.parents.len() != 1
        || !commit.parents[0].sha.eq_ignore_ascii_case(parent_id)
    {
        return Err(GithubBrokerError::AuthorityDrift);
    }
    Ok(())
}

fn validate_ref(
    reference: &RefWire,
    branch: &str,
    commit_id: &str,
) -> Result<(), GithubBrokerError> {
    if reference.reference != format!("refs/heads/{branch}")
        || reference.object.kind != "commit"
        || !reference.object.sha.eq_ignore_ascii_case(commit_id)
    {
        return Err(GithubBrokerError::AmbiguousWrite);
    }
    Ok(())
}

fn same_connection(expected: &ConnectionSnapshot, current: &ConnectionSnapshot) -> bool {
    expected.service_id == current.service_id
        && expected.service_state_version == current.service_state_version
        && expected.backing_key_id == current.backing_key_id
        && expected.account == current.account
        && expected.granted_scopes == current.granted_scopes
        && expected.satisfied_scope_classes == current.satisfied_scope_classes
        && current.state == layer2_core::ConnectionState::Active
}
