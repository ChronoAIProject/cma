//! Guarded Git Data publication for one explicitly confirmed Trigger diff.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine as _;
use layer2_core::{
    ConnectionSnapshot, ContributionRecord, DiffChangeKind, ForkRecord, Launch,
    PublicationObjectPhase, PublicationObjectWrite, PublicationState, RepositoryIdentity,
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
pub(crate) struct PendingGitData {
    pub(crate) base_tree_id: String,
    pub(crate) tree_id: Option<String>,
    pub(crate) object_ids: Vec<String>,
    pub(crate) object_write: PublicationObjectWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PublicationAdvance {
    ObjectWritePending(PendingGitData),
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
    author: CommitIdentityWire,
    committer: CommitIdentityWire,
}

#[derive(Debug, Deserialize)]
struct CommitIdentityWire {
    name: String,
    email: String,
    date: String,
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

struct PublicationRecipe {
    tree_entries: Vec<Value>,
    object_ids: Vec<String>,
    blob_file_indexes: Vec<usize>,
}

impl GithubBroker {
    pub(crate) async fn publish_branch(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
    ) -> Result<PublicationAdvance, GithubBrokerError> {
        validate_plan(context, plan)?;
        reject_lfs(plan)?;
        let repository = self.current_publication_write_fork(context, plan).await?;
        match context.contribution.state {
            PublicationState::IntentRecorded if context.contribution.object_write.is_none() => self
                .prepare_publication_objects(context, plan, &repository)
                .await
                .map(PublicationAdvance::ObjectWritePending),
            PublicationState::IntentRecorded => {
                self.advance_publication_objects(context, plan, &repository)
                    .await
            }
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
                validate_commit_response(
                    &commit,
                    &prepared.tree_id,
                    &plan.expected_head,
                    context,
                    plan,
                )?;
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

    /// Observe a previously admitted publication attempt without issuing any
    /// blob, tree, commit, or ref write. An uncommitted object recipe is
    /// reconstructed from immutable review data because no object write is
    /// admitted until that recipe itself is durable.
    pub(crate) async fn reconcile_branch_publication(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
    ) -> Result<PublicationAdvance, GithubBrokerError> {
        validate_plan(context, plan)?;
        reject_lfs(plan)?;
        let repository = self.current_publication_fork(context, plan).await?;
        match context.contribution.state {
            PublicationState::IntentRecorded if context.contribution.object_write.is_none() => self
                .prepare_publication_objects(context, plan, &repository)
                .await
                .map(PublicationAdvance::ObjectWritePending),
            PublicationState::IntentRecorded => self
                .reconcile_publication_objects(context, plan, &repository)
                .await
                .map(PublicationAdvance::ObjectWritePending),
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
                validate_commit_response(
                    &commit,
                    &prepared.tree_id,
                    &plan.expected_head,
                    context,
                    plan,
                )?;
                Ok(PublicationAdvance::RefUpdatePending(prepared))
            }
            PublicationState::RefUpdatePending => {
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
                    self.verified_result(
                        context,
                        plan,
                        prepared.base_tree_id,
                        prepared.tree_id,
                        prepared.commit_id,
                        prepared.object_ids,
                    )
                    .await
                    .map(PublicationAdvance::Published)
                } else if observed_head.eq_ignore_ascii_case(&plan.expected_head) {
                    Err(GithubBrokerError::AmbiguousWrite)
                } else {
                    Err(GithubBrokerError::BranchHeadConflict)
                }
            }
            _ => Err(GithubBrokerError::Malformed(
                "the publication state has no unsettled write to reconcile".into(),
            )),
        }
    }

    async fn prepare_publication_objects(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        repository: &layer2_core::RepositoryIdentity,
    ) -> Result<PendingGitData, GithubBrokerError> {
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
        let recipe = publication_recipe(plan)?;
        let request_digest = object_request_digest(context, plan, &base_tree_id, &recipe)?;
        Ok(PendingGitData {
            base_tree_id,
            tree_id: None,
            object_ids: recipe.object_ids,
            object_write: PublicationObjectWrite {
                version: PublicationObjectWrite::VERSION,
                request_digest,
                phase: PublicationObjectPhase::BlobsPending,
            },
        })
    }

    async fn advance_publication_objects(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        repository: &layer2_core::RepositoryIdentity,
    ) -> Result<PublicationAdvance, GithubBrokerError> {
        let (mut pending, recipe) = retained_object_write(context, plan)?;
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

        match pending.object_write.phase {
            PublicationObjectPhase::BlobsPending => {
                for file_index in &recipe.blob_file_indexes {
                    let file = &plan.files[*file_index];
                    let blob_id = file.blob_id.as_deref().ok_or_else(|| {
                        GithubBrokerError::Malformed(
                            "a retained blob write has no Git object identity".into(),
                        )
                    })?;
                    if self
                        .publication_blob_exists(context, repository, blob_id)
                        .await?
                    {
                        continue;
                    }
                    self.write_publication_blob(context, plan, file, blob_id)
                        .await?;
                    return Ok(PublicationAdvance::ObjectWritePending(pending));
                }
                pending.object_write.phase = PublicationObjectPhase::TreePending;
                Ok(PublicationAdvance::ObjectWritePending(pending))
            }
            PublicationObjectPhase::TreePending => {
                let tree_id = self
                    .write_publication_tree(context, plan, &pending.base_tree_id, &recipe)
                    .await?;
                pending.tree_id = Some(tree_id);
                pending.object_write.phase = PublicationObjectPhase::CommitPending;
                Ok(PublicationAdvance::ObjectWritePending(pending))
            }
            PublicationObjectPhase::CommitPending => {
                let tree_id = pending.tree_id.as_deref().ok_or_else(|| {
                    GithubBrokerError::Malformed(
                        "the retained commit write has no resulting tree".into(),
                    )
                })?;
                if !self
                    .publication_tree_exists(context, repository, tree_id)
                    .await?
                {
                    let recreated = self
                        .write_publication_tree(context, plan, &pending.base_tree_id, &recipe)
                        .await?;
                    if !recreated.eq_ignore_ascii_case(tree_id) {
                        return Err(GithubBrokerError::AuthorityDrift);
                    }
                    return Ok(PublicationAdvance::ObjectWritePending(pending));
                }
                let commit_id = self
                    .write_publication_commit(context, plan, tree_id)
                    .await?;
                Ok(PublicationAdvance::ObjectsWritten(PreparedGitData {
                    base_tree_id: pending.base_tree_id,
                    tree_id: tree_id.to_string(),
                    commit_id,
                    object_ids: pending.object_ids,
                }))
            }
        }
    }

    async fn reconcile_publication_objects(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        repository: &layer2_core::RepositoryIdentity,
    ) -> Result<PendingGitData, GithubBrokerError> {
        let (mut pending, recipe) = retained_object_write(context, plan)?;
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

        if pending.object_write.phase == PublicationObjectPhase::BlobsPending {
            for file_index in &recipe.blob_file_indexes {
                let blob_id = plan.files[*file_index].blob_id.as_deref().ok_or_else(|| {
                    GithubBrokerError::Malformed(
                        "a retained blob write has no Git object identity".into(),
                    )
                })?;
                if !self
                    .publication_blob_exists(context, repository, blob_id)
                    .await?
                {
                    // Absence is a positive pre-write proof for this
                    // content-addressed object. Queue a freshly admitted exact
                    // reconstruction instead of spending reconciliation budget.
                    return Ok(pending);
                }
            }
            pending.object_write.phase = PublicationObjectPhase::TreePending;
        } else if pending.object_write.phase == PublicationObjectPhase::CommitPending {
            let tree_id = pending.tree_id.as_deref().ok_or_else(|| {
                GithubBrokerError::Malformed("the retained commit has no tree identity".into())
            })?;
            let _ = self
                .publication_tree_exists(context, repository, tree_id)
                .await?;
        }

        // Git tree and commit creation are immutable, content-addressed writes.
        // GitHub offers no lookup by request body, so an unknown response is
        // resumed only from this exact digest-sealed recipe under a fresh write
        // admission; no ref is changed in this phase.
        Ok(pending)
    }

    async fn write_publication_blob(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        file: &PublicationFile,
        blob_id: &str,
    ) -> Result<(), GithubBrokerError> {
        let content = file.content.as_deref().ok_or_else(|| {
            GithubBrokerError::Malformed("a retained blob write has no bytes".into())
        })?;
        let repository = self.current_publication_write_fork(context, plan).await?;
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
        Ok(())
    }

    async fn write_publication_tree(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        base_tree_id: &str,
        recipe: &PublicationRecipe,
    ) -> Result<String, GithubBrokerError> {
        let repository = self.current_publication_write_fork(context, plan).await?;
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
                    "tree": recipe.tree_entries,
                })),
            )
            .await?;
        validate_commit_sha(&tree.value.sha)?;
        Ok(tree.value.sha.to_ascii_lowercase())
    }

    async fn write_publication_commit(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        tree_id: &str,
    ) -> Result<String, GithubBrokerError> {
        let repository = self.current_publication_write_fork(context, plan).await?;
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
                Some(commit_request(context, plan, tree_id)),
            )
            .await?;
        validate_commit_response(&commit.value, tree_id, &plan.expected_head, context, plan)?;
        Ok(commit.value.sha.to_ascii_lowercase())
    }

    async fn publication_blob_exists(
        &self,
        context: &PublicationContext,
        repository: &RepositoryIdentity,
        blob_id: &str,
    ) -> Result<bool, GithubBrokerError> {
        validate_commit_sha(blob_id)?;
        let blob: Result<Observed<BlobWire>, GithubBrokerError> = self
            .github_json(
                &context.owner,
                &context.connection.service_id,
                Method::GET,
                &[
                    "repos",
                    repository.canonical_owner.as_str(),
                    repository.canonical_name.as_str(),
                    "git",
                    "blobs",
                    blob_id,
                ],
                &[],
                None,
            )
            .await;
        match blob {
            Ok(blob) if blob.value.sha.eq_ignore_ascii_case(blob_id) => Ok(true),
            Ok(_) => Err(GithubBrokerError::AuthorityDrift),
            Err(GithubBrokerError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn publication_tree_exists(
        &self,
        context: &PublicationContext,
        repository: &RepositoryIdentity,
        tree_id: &str,
    ) -> Result<bool, GithubBrokerError> {
        validate_commit_sha(tree_id)?;
        let tree: Result<Observed<TreeReadWire>, GithubBrokerError> = self
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
                &[],
                None,
            )
            .await;
        match tree {
            Ok(_) => Ok(true),
            Err(GithubBrokerError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
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
                .verified_result(context, plan, base_tree_id, tree_id, commit_id, object_ids)
                .await;
        }
        if !before_patch.eq_ignore_ascii_case(&plan.expected_head) {
            return Err(GithubBrokerError::BranchHeadConflict);
        }

        // Re-establish authority again at the final ref-update boundary.
        let repository = self.current_publication_write_fork(context, plan).await?;
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
                        base_tree_id,
                        tree_id,
                        commit_id,
                        object_ids,
                        error,
                    )
                    .await
            }
        }
        self.verified_result(context, plan, base_tree_id, tree_id, commit_id, object_ids)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn reconcile_patch_error(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
        base_tree_id: String,
        tree_id: String,
        commit_id: String,
        object_ids: Vec<String>,
        error: GithubBrokerError,
    ) -> Result<PublishedGitData, GithubBrokerError> {
        let repository = self.current_publication_fork(context, plan).await?;
        let observed = self
            .resolve_branch_sha(
                &context.owner,
                &context.connection.service_id,
                &repository,
                &context.branch_name,
            )
            .await;
        match observed {
            Ok(head) if head.eq_ignore_ascii_case(&commit_id) => {
                self.verified_result(context, plan, base_tree_id, tree_id, commit_id, object_ids)
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
        base_tree_id: String,
        tree_id: String,
        commit_id: String,
        object_ids: Vec<String>,
    ) -> Result<PublishedGitData, GithubBrokerError> {
        let repository = self.current_publication_fork(context, plan).await?;
        self.verify_publication(
            context,
            plan,
            &repository,
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

    async fn current_publication_fork(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
    ) -> Result<RepositoryIdentity, GithubBrokerError> {
        self.current_branch_fork(&BranchContext {
            owner: context.owner.clone(),
            launch_id: context.launch_id.clone(),
            connection: context.connection.clone(),
            fork: context.fork.clone(),
            source_sha: plan.expected_head.clone(),
        })
        .await
    }

    /// Re-establish both delegated service authority and numeric repository
    /// identity at the boundary of every Git Data write. The subject bearer is
    /// still resolved independently by `github_json` for the following request.
    async fn current_publication_write_fork(
        &self,
        context: &PublicationContext,
        plan: &PublicationPlan,
    ) -> Result<RepositoryIdentity, GithubBrokerError> {
        let current = self
            .preflight_connection(&context.owner, &context.connection.service_id)
            .await?;
        if !same_connection(&context.connection, &current.snapshot) {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        self.current_publication_fork(context, plan).await
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
        validate_commit_response(&commit, tree_id, &plan.expected_head, context, plan)?;
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

fn publication_recipe(plan: &PublicationPlan) -> Result<PublicationRecipe, GithubBrokerError> {
    let mut inline_bytes = 0_usize;
    let mut tree_entries = Vec::with_capacity(plan.files.len());
    let mut object_ids = BTreeSet::new();
    let mut blob_file_indexes = Vec::new();
    for (file_index, file) in plan.files.iter().enumerate() {
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
                blob_file_indexes.push(file_index);
                json!({
                    "path": file.path,
                    "mode": file.mode,
                    "type": "blob",
                    "sha": blob_id,
                })
            }
        };
        tree_entries.push(entry);
    }
    Ok(PublicationRecipe {
        tree_entries,
        object_ids: object_ids.into_iter().collect(),
        blob_file_indexes,
    })
}

fn publication_message(context: &PublicationContext, plan: &PublicationPlan) -> String {
    format!(
        "CMA Trigger contribution\n\nLaunch: {}\nReviewed diff: {}",
        context.launch_id, plan.diff_digest
    )
}

fn commit_request(context: &PublicationContext, plan: &PublicationPlan, tree_id: &str) -> Value {
    json!({
        "message": publication_message(context, plan),
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
    })
}

fn object_request_digest(
    context: &PublicationContext,
    plan: &PublicationPlan,
    base_tree_id: &str,
    recipe: &PublicationRecipe,
) -> Result<String, GithubBrokerError> {
    let blob_writes = recipe
        .blob_file_indexes
        .iter()
        .map(|index| {
            let file = &plan.files[*index];
            json!({
                "path": file.path,
                "blob_id": file.blob_id,
                "content_sha256": file.content_sha256,
            })
        })
        .collect::<Vec<_>>();
    let sealed = json!({
        "cma_git_data_object_recipe": PublicationObjectWrite::VERSION,
        "base_tree": base_tree_id,
        "blob_writes": blob_writes,
        "tree": &recipe.tree_entries,
        "commit": {
            "message": publication_message(context, plan),
            "tree": "result_of_sealed_tree_request",
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
        },
    });
    let bytes = serde_json::to_vec(&sealed).map_err(|error| {
        GithubBrokerError::Malformed(format!("seal Git object recipe: {error}"))
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn retained_object_write(
    context: &PublicationContext,
    plan: &PublicationPlan,
) -> Result<(PendingGitData, PublicationRecipe), GithubBrokerError> {
    let base_tree_id = context.contribution.base_tree_id.clone().ok_or_else(|| {
        GithubBrokerError::Malformed("the retained object write has no base tree".into())
    })?;
    validate_commit_sha(&base_tree_id)?;
    if let Some(tree_id) = context.contribution.tree_id.as_deref() {
        validate_commit_sha(tree_id)?;
    }
    let object_write = context.contribution.object_write.clone().ok_or_else(|| {
        GithubBrokerError::Malformed("the retained publication has no object-write cursor".into())
    })?;
    let recipe = publication_recipe(plan)?;
    let expected_digest = object_request_digest(context, plan, &base_tree_id, &recipe)?;
    if object_write.version != PublicationObjectWrite::VERSION
        || object_write.request_digest != expected_digest
        || context.contribution.object_ids != recipe.object_ids
    {
        return Err(GithubBrokerError::AuthorityDrift);
    }
    Ok((
        PendingGitData {
            base_tree_id,
            tree_id: context.contribution.tree_id.clone(),
            object_ids: context.contribution.object_ids.clone(),
            object_write,
        },
        recipe,
    ))
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
    context: &PublicationContext,
    plan: &PublicationPlan,
) -> Result<(), GithubBrokerError> {
    validate_commit_sha(&commit.sha)?;
    let expected_date = &context.contribution.confirmation.confirmed_at;
    if !commit.tree.sha.eq_ignore_ascii_case(tree_id)
        || commit.parents.len() != 1
        || !commit.parents[0].sha.eq_ignore_ascii_case(parent_id)
        || commit.author.name != plan.author.name
        || commit.author.email != plan.author.email
        || commit.author.date != *expected_date
        || commit.committer.name != plan.committer.name
        || commit.committer.email != plan.committer.email
        || commit.committer.date != *expected_date
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
        && expected.account.account_id == current.account.account_id
        && expected.account.account_node_id == current.account.account_node_id
        && expected.granted_scopes.iter().collect::<BTreeSet<_>>()
            == current.granted_scopes.iter().collect::<BTreeSet<_>>()
        && expected
            .satisfied_scope_classes
            .iter()
            .collect::<BTreeSet<_>>()
            == current
                .satisfied_scope_classes
                .iter()
                .collect::<BTreeSet<_>>()
        && current.state == layer2_core::ConnectionState::Active
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{CommitIdentity, DiffArtifactId, LaunchId};

    fn publication_plan(path: &str, content: &[u8]) -> PublicationPlan {
        PublicationPlan {
            launch_id: LaunchId("lnch_lfs_fixture".into()),
            artifact_id: DiffArtifactId("artifact-lfs-fixture".into()),
            revision_digest: "a".repeat(64),
            baseline_digest: "b".repeat(64),
            checkpoint_digest: "c".repeat(64),
            diff_digest: "d".repeat(64),
            expected_head: "e".repeat(40),
            author: CommitIdentity {
                name: "Contributor".into(),
                email: "contributor@example.test".into(),
            },
            committer: CommitIdentity {
                name: "CMA".into(),
                email: "cma@chrono.ai".into(),
            },
            files: vec![PublicationFile {
                path: path.into(),
                change: DiffChangeKind::Added,
                binary: false,
                mode: "100644".into(),
                content_sha256: Some(format!("{:x}", Sha256::digest(content))),
                blob_id: Some(git_blob_id(content)),
                content: Some(content.to_vec()),
            }],
        }
    }

    #[test]
    fn publication_refuses_new_lfs_attributes_and_pointer_content() {
        for (path, content) in [
            (
                ".gitattributes",
                b"*.bin FILTER=LFS diff=lfs merge=lfs -text\n".as_slice(),
            ),
            (
                "large.bin",
                b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\n".as_slice(),
            ),
        ] {
            assert!(matches!(
                reject_lfs(&publication_plan(path, content)),
                Err(GithubBrokerError::UnsupportedWorkspace(
                    WorkspaceUnsupported::LargeFileStorage
                ))
            ));
        }

        reject_lfs(&publication_plan(
            ".gitattributes",
            b"*.txt export-subst eol=lf filter=ordinary\n",
        ))
        .expect("non-LFS attributes remain inert repository content");
    }
}
