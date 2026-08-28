use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{
    Confirmation, ConfirmationAction, ContributionId, DiffArtifactId, LaunchFailure, LaunchId,
    TriggerValidationError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitIdentity {
    pub name: String,
    pub email: String,
}

impl CommitIdentity {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.name.trim().is_empty()
            || self.name.len() > 256
            || self.email.trim().is_empty()
            || self.email.len() > 320
            || !self.email.contains('@')
            || self.email.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(TriggerValidationError::field(
                "commit_identity",
                "is not a usable public commit identity",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationState {
    IntentRecorded,
    ObjectsWritten,
    RefUpdatePending,
    Published,
    Conflict,
    RecoveryRequired,
    Failed,
}

/// Durable cursor for reconstructing one immutable Git Data object recipe.
///
/// The cursor is recorded before any object POST. A response-loss recovery may
/// therefore observe a content-addressed blob or repeat the exact sealed tree
/// or commit request without inventing a new publication intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationObjectPhase {
    BlobsPending,
    TreePending,
    CommitPending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicationObjectWrite {
    pub version: u32,
    pub request_digest: String,
    pub phase: PublicationObjectPhase,
}

impl PublicationObjectWrite {
    pub const VERSION: u32 = 1;

    fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.version != Self::VERSION {
            return Err(TriggerValidationError::field(
                "contribution.object_write.version",
                "is not a supported Git object recipe version",
            ));
        }
        validate_sha256(
            "contribution.object_write.request_digest",
            &self.request_digest,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributionRecord {
    pub id: ContributionId,
    pub launch_id: LaunchId,
    pub diff_artifact_id: DiffArtifactId,
    pub revision_digest: String,
    pub baseline_digest: String,
    pub checkpoint_digest: String,
    pub diff_digest: String,
    pub branch_name: String,
    pub state: PublicationState,
    pub expected_head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_tree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resulting_head: Option<String>,
    #[serde(default)]
    pub object_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_write: Option<PublicationObjectWrite>,
    pub author: CommitIdentity,
    pub committer: CommitIdentity,
    pub confirmation: Confirmation,
    pub intent_key: String,
    pub created_at: String,
    pub updated_at: String,
}

impl ContributionRecord {
    pub fn validate_intent(&self, launch: &LaunchId) -> Result<(), TriggerValidationError> {
        if &self.launch_id != launch
            || self.confirmation.action != ConfirmationAction::PublishBranch
            || self.confirmation.subject.0.trim().is_empty()
            || self.branch_name.trim().is_empty()
            || self.intent_key.trim().is_empty()
        {
            return Err(TriggerValidationError::field(
                "contribution",
                "is not a valid publication intent for this Launch",
            ));
        }
        validate_sha256("contribution.revision_digest", &self.revision_digest)?;
        validate_sha256("contribution.baseline_digest", &self.baseline_digest)?;
        validate_sha256("contribution.checkpoint_digest", &self.checkpoint_digest)?;
        validate_sha256("contribution.diff_digest", &self.diff_digest)?;
        validate_git_id("contribution.expected_head", &self.expected_head)?;
        for (field, value) in [
            ("contribution.base_tree_id", self.base_tree_id.as_deref()),
            ("contribution.tree_id", self.tree_id.as_deref()),
            ("contribution.commit_id", self.commit_id.as_deref()),
            (
                "contribution.resulting_head",
                self.resulting_head.as_deref(),
            ),
        ] {
            if let Some(value) = value {
                validate_git_id(field, value)?;
            }
        }
        if self.object_ids.len() > super::MAX_DIFF_SUMMARY_FILES {
            return Err(TriggerValidationError::field(
                "contribution.object_ids",
                "exceeds the bounded publication object inventory",
            ));
        }
        for object in &self.object_ids {
            validate_git_id("contribution.object_ids", object)?;
        }
        if self
            .object_ids
            .windows(2)
            .any(|pair| pair[0].as_str() >= pair[1].as_str())
        {
            return Err(TriggerValidationError::field(
                "contribution.object_ids",
                "must be a sorted unique Git object inventory",
            ));
        }
        let has_complete_objects =
            self.base_tree_id.is_some() && self.tree_id.is_some() && self.commit_id.is_some();
        let has_partial_objects = self.base_tree_id.is_some()
            || self.tree_id.is_some()
            || self.commit_id.is_some()
            || !self.object_ids.is_empty();
        match self.state {
            PublicationState::IntentRecorded => match self.object_write.as_ref() {
                None if has_partial_objects || self.resulting_head.is_some() => {
                    return Err(TriggerValidationError::field(
                        "contribution.state",
                        "an unprepared intent cannot claim Git object or ref evidence",
                    ));
                }
                Some(write) => {
                    write.validate()?;
                    let valid_shape = self.base_tree_id.is_some()
                        && self.commit_id.is_none()
                        && self.resulting_head.is_none()
                        && match write.phase {
                            PublicationObjectPhase::BlobsPending
                            | PublicationObjectPhase::TreePending => self.tree_id.is_none(),
                            PublicationObjectPhase::CommitPending => self.tree_id.is_some(),
                        };
                    if !valid_shape {
                        return Err(TriggerValidationError::field(
                            "contribution.object_write",
                            "does not match its retained Git object phase",
                        ));
                    }
                }
                None => {}
            },
            PublicationState::ObjectsWritten | PublicationState::RefUpdatePending
                if !has_complete_objects
                    || self.resulting_head.is_some()
                    || self.object_write.is_some() =>
            {
                return Err(TriggerValidationError::field(
                    "contribution.state",
                    "prepared publication requires complete Git object evidence and no resulting ref",
                ));
            }
            PublicationState::Published
                if !has_complete_objects
                    || self.resulting_head != self.commit_id
                    || self.object_write.is_some() =>
            {
                return Err(TriggerValidationError::field(
                    "contribution.state",
                    "published requires the exact base tree, resulting tree, commit, and ref head",
                ));
            }
            PublicationState::Conflict
            | PublicationState::RecoveryRequired
            | PublicationState::Failed
                if has_partial_objects != has_complete_objects
                    || self.object_write.is_some()
                    || self.resulting_head.as_ref().is_some_and(|head| {
                        self.commit_id
                            .as_ref()
                            .is_none_or(|commit| !head.eq_ignore_ascii_case(commit))
                    }) =>
            {
                return Err(TriggerValidationError::field(
                    "contribution.state",
                    "terminal publication evidence is incomplete or inconsistent",
                ));
            }
            _ => {}
        }
        self.author.validate()?;
        self.committer.validate()
    }
}

fn validate_sha256(field: &str, value: &str) -> Result<(), TriggerValidationError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TriggerValidationError::field(
            field,
            "must be a complete SHA-256 digest",
        ));
    }
    Ok(())
}

fn validate_git_id(field: &str, value: &str) -> Result<(), TriggerValidationError> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TriggerValidationError::field(
            field,
            "must be a complete Git object ID",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mergeability {
    Computing,
    Mergeable,
    Conflicting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestState {
    Creating,
    Updating,
    Open,
    ClosedUnmerged,
    Merged,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckSummaryState {
    Pending,
    Passing,
    Failing,
    AwaitingMaintainerApproval,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestOperation {
    Open,
    Update,
}

/// Durable cadence and failure evidence for read-only provider status refresh.
///
/// This state never authorizes a pull-request write. It exists so an in-process
/// scheduler can resume status observation after a browser closes or a CMA
/// replica restarts without polling GitHub on every scheduler tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PullRequestStatusSync {
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<LaunchFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_matches_launch: Option<bool>,
}

impl PullRequestOperation {
    pub fn confirmation_action(self) -> ConfirmationAction {
        match self {
            Self::Open => ConfirmationAction::OpenPullRequest,
            Self::Update => ConfirmationAction::UpdatePullRequest,
        }
    }

    pub(crate) fn wire_name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Update => "update",
        }
    }
}

/// Exact pull-request write the contributor reviews before CMA records it.
///
/// Provider observations deliberately live outside this value. This makes the
/// user-authorized target and content immutable while reconciliation fills in
/// the provider result independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestDraft {
    pub launch_id: LaunchId,
    pub operation: PullRequestOperation,
    pub marker: String,
    pub upstream_repository_id: u64,
    pub upstream_repository_node_id: String,
    pub fork_repository_id: u64,
    pub fork_repository_node_id: String,
    pub base_ref: String,
    pub head_owner: String,
    pub head_branch: String,
    pub head_ref: String,
    pub expected_base_revision: String,
    pub expected_head_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_head_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_provider_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_provider_node_id: Option<String>,
    /// Update the PR title/body only when the contributor explicitly edited
    /// them and the previously observed provider content still matches.
    pub update_metadata: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_previous_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_previous_body_digest: Option<String>,
    pub title: String,
    /// Human-visible body without the hidden CMA marker.
    pub body: String,
    /// Digest of the exact provider body, including the hidden marker.
    pub body_digest: String,
    /// Digest presented in the contribution review and bound by confirmation.
    pub intent_digest: String,
}

impl PullRequestDraft {
    #[allow(clippy::too_many_arguments)]
    pub fn seal(
        launch_id: LaunchId,
        operation: PullRequestOperation,
        upstream_repository_id: u64,
        upstream_repository_node_id: String,
        fork_repository_id: u64,
        fork_repository_node_id: String,
        base_ref: String,
        head_owner: String,
        head_branch: String,
        expected_base_revision: String,
        expected_head_revision: String,
        previous_head_revision: Option<String>,
        target_number: Option<u64>,
        target_provider_id: Option<u64>,
        target_provider_node_id: Option<String>,
        update_metadata: bool,
        expected_previous_title: Option<String>,
        expected_previous_body_digest: Option<String>,
        title: String,
        body: String,
    ) -> Result<Self, TriggerValidationError> {
        let marker = pull_request_marker(&launch_id);
        let head_ref = format!("{head_owner}:{head_branch}");
        let body_digest = pull_request_body_digest(&body, &marker);
        let mut draft = Self {
            launch_id,
            operation,
            marker,
            upstream_repository_id,
            upstream_repository_node_id,
            fork_repository_id,
            fork_repository_node_id,
            base_ref,
            head_owner,
            head_branch,
            head_ref,
            expected_base_revision,
            expected_head_revision,
            previous_head_revision,
            target_number,
            target_provider_id,
            target_provider_node_id,
            update_metadata,
            expected_previous_title,
            expected_previous_body_digest,
            title,
            body,
            body_digest,
            intent_digest: String::new(),
        };
        draft.intent_digest = draft.compute_intent_digest();
        draft.validate()?;
        Ok(draft)
    }

    pub fn provider_body(&self) -> String {
        provider_pull_request_body(&self.body, &self.marker)
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.upstream_repository_id == 0
            || self.fork_repository_id == 0
            || !valid_bounded_text(&self.upstream_repository_node_id, 255)
            || !valid_bounded_text(&self.fork_repository_node_id, 255)
            || !valid_ref(&self.base_ref)
            || !valid_owner(&self.head_owner)
            || !valid_ref(&self.head_branch)
            || self.head_branch != format!("cma/{}", self.launch_id.0)
            || self.head_ref != format!("{}:{}", self.head_owner, self.head_branch)
            || self.marker != pull_request_marker(&self.launch_id)
        {
            return Err(TriggerValidationError::field(
                "pull_request",
                "does not bind the exact upstream, fork, base, and CMA head",
            ));
        }
        validate_git_id(
            "pull_request.expected_base_revision",
            &self.expected_base_revision,
        )?;
        validate_git_id(
            "pull_request.expected_head_revision",
            &self.expected_head_revision,
        )?;
        if self.title.trim().is_empty()
            || self.title.len() > 256
            || self.title.bytes().any(|byte| byte.is_ascii_control())
            || self.body.len() > 65_536
            || self.body.contains("<!-- cma-trigger:")
            || self
                .body
                .bytes()
                .any(|byte| byte.is_ascii_control() && !matches!(byte, b'\n' | b'\r' | b'\t'))
        {
            return Err(TriggerValidationError::field(
                "pull_request.content",
                "is outside the approved title/body shape",
            ));
        }
        if self.body_digest != pull_request_body_digest(&self.body, &self.marker)
            || self.intent_digest != self.compute_intent_digest()
        {
            return Err(TriggerValidationError::field(
                "pull_request.intent_digest",
                "does not seal the exact pull-request content and target",
            ));
        }
        match self.operation {
            PullRequestOperation::Open
                if self.previous_head_revision.is_some()
                    || self.target_number.is_some()
                    || self.target_provider_id.is_some()
                    || self.target_provider_node_id.is_some()
                    || self.update_metadata
                    || self.expected_previous_title.is_some()
                    || self.expected_previous_body_digest.is_some() =>
            {
                return Err(TriggerValidationError::field(
                    "pull_request.operation",
                    "an open intent cannot claim an existing provider pull request",
                ));
            }
            PullRequestOperation::Update => {
                let previous = self.previous_head_revision.as_deref().ok_or_else(|| {
                    TriggerValidationError::field(
                        "pull_request.previous_head_revision",
                        "an update must bind the previously observed pull-request head",
                    )
                })?;
                validate_git_id("pull_request.previous_head_revision", previous)?;
                if previous.eq_ignore_ascii_case(&self.expected_head_revision)
                    || self.target_number.is_none_or(|number| number == 0)
                    || self.target_provider_id.is_none_or(|id| id == 0)
                    || self
                        .target_provider_node_id
                        .as_deref()
                        .is_none_or(|node| !valid_bounded_text(node, 255))
                {
                    return Err(TriggerValidationError::field(
                        "pull_request.operation",
                        "an update must target one existing PR and a new reviewed head",
                    ));
                }
                if self.update_metadata {
                    if self
                        .expected_previous_title
                        .as_deref()
                        .is_none_or(|title| title.trim().is_empty() || title.len() > 256)
                        || self
                            .expected_previous_body_digest
                            .as_deref()
                            .is_none_or(|digest| !is_sha256(digest))
                    {
                        return Err(TriggerValidationError::field(
                            "pull_request.update_metadata",
                            "an explicit metadata edit must bind the previously observed title and body",
                        ));
                    }
                } else if self.expected_previous_title.is_some()
                    || self.expected_previous_body_digest.is_some()
                {
                    return Err(TriggerValidationError::field(
                        "pull_request.update_metadata",
                        "metadata expectations are valid only for an explicit metadata edit",
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn compute_intent_digest(&self) -> String {
        let mut digest = Sha256::new();
        let fields = vec![
            self.launch_id.0.clone(),
            self.operation.wire_name().to_string(),
            self.marker.clone(),
            self.upstream_repository_id.to_string(),
            self.upstream_repository_node_id.clone(),
            self.fork_repository_id.to_string(),
            self.fork_repository_node_id.clone(),
            self.base_ref.clone(),
            self.head_owner.clone(),
            self.head_branch.clone(),
            self.head_ref.clone(),
            self.expected_base_revision.clone(),
            self.expected_head_revision.clone(),
            self.previous_head_revision.clone().unwrap_or_default(),
            self.target_number.unwrap_or_default().to_string(),
            self.target_provider_id.unwrap_or_default().to_string(),
            self.target_provider_node_id.clone().unwrap_or_default(),
            self.update_metadata.to_string(),
            self.expected_previous_title.clone().unwrap_or_default(),
            self.expected_previous_body_digest
                .clone()
                .unwrap_or_default(),
            self.title.clone(),
            self.body_digest.clone(),
        ];
        for value in fields {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        format!("{:x}", digest.finalize())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestRecord {
    pub intent: PullRequestDraft,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_url: Option<String>,
    pub state: PullRequestState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_body_digest: Option<String>,
    pub mergeability: Mergeability,
    pub checks: CheckSummaryState,
    pub confirmation: Confirmation,
    pub intent_key: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_updated_at: Option<String>,
    /// Internal read-only provider refresh state. Public projections expose
    /// the resulting PR fields and safe failure, never this worker cadence.
    #[serde(default)]
    pub status_sync: PullRequestStatusSync,
}

impl PullRequestRecord {
    pub fn validate_intent(&self, launch: &LaunchId) -> Result<(), TriggerValidationError> {
        self.intent.validate()?;
        if &self.intent.launch_id != launch
            || self.confirmation.action != self.intent.operation.confirmation_action()
            || self.confirmation.snapshot_digest != self.intent.intent_digest
            || self.confirmation.subject.0.trim().is_empty()
            || self.intent_key
                != format!(
                    "cma-trigger:{}:pr:{}:{}",
                    launch.0,
                    self.intent.operation.wire_name(),
                    self.intent.intent_digest
                )
            || self.created_at != self.confirmation.confirmed_at
        {
            return Err(TriggerValidationError::field(
                "pull_request",
                "is not a valid explicit pull-request intent for this Launch",
            ));
        }
        let complete_provider_identity = self.provider_id.is_some_and(|id| id > 0)
            && self
                .provider_node_id
                .as_deref()
                .is_some_and(|value| valid_bounded_text(value, 255))
            && self.number.is_some_and(|number| number > 0)
            && self
                .canonical_url
                .as_deref()
                .is_some_and(valid_provider_url)
            && self.base_revision.as_deref().is_some_and(is_git_id)
            && self.head_revision.as_deref().is_some_and(is_git_id)
            && self
                .observed_title
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty() && value.len() <= 256)
            && self.observed_body_digest.as_deref().is_some_and(is_sha256)
            && self
                .provider_created_at
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            && self
                .provider_updated_at
                .as_deref()
                .is_some_and(|value| !value.is_empty());
        match self.state {
            PullRequestState::Creating if self.intent.operation != PullRequestOperation::Open => {
                return Err(TriggerValidationError::field(
                    "pull_request.state",
                    "creating requires an open intent",
                ));
            }
            PullRequestState::Creating if has_any_provider_identity(self) => {
                return Err(TriggerValidationError::field(
                    "pull_request.state",
                    "creating cannot claim provider result evidence",
                ));
            }
            PullRequestState::Updating
                if self.intent.operation != PullRequestOperation::Update
                    || !complete_provider_identity
                    || self.number != self.intent.target_number
                    || self.provider_id != self.intent.target_provider_id
                    || self.provider_node_id != self.intent.target_provider_node_id =>
            {
                return Err(TriggerValidationError::field(
                    "pull_request.state",
                    "updating must retain the exact existing provider PR",
                ));
            }
            PullRequestState::Open
            | PullRequestState::ClosedUnmerged
            | PullRequestState::Merged
                if !complete_provider_identity =>
            {
                return Err(TriggerValidationError::field(
                    "pull_request.state",
                    "a reconciled state requires complete provider evidence",
                ));
            }
            PullRequestState::RecoveryRequired => {}
            _ => {}
        }
        Ok(())
    }
}

pub fn pull_request_marker(launch: &LaunchId) -> String {
    format!("<!-- cma-trigger:launch:{}:v1 -->", launch.0)
}

pub fn provider_pull_request_body(body: &str, marker: &str) -> String {
    if body.trim().is_empty() {
        marker.to_string()
    } else {
        format!("{}\n\n{marker}", body.trim_end())
    }
}

pub fn pull_request_body_digest(body: &str, marker: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(provider_pull_request_body(body, marker))
    )
}

fn valid_bounded_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn valid_owner(value: &str) -> bool {
    valid_bounded_text(value, 255)
        && !value.contains('/')
        && !value.contains(':')
        && !value.contains("..")
}

fn valid_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("..")
        && !value.contains("@{")
        && !value.bytes().any(|byte| {
            byte.is_ascii_control()
                || matches!(byte, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
        && value
            .split('/')
            .all(|component| !component.is_empty() && !matches!(component, "." | ".."))
}

fn valid_provider_url(value: &str) -> bool {
    value.starts_with("https://github.com/")
        && value.len() <= 2_048
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn is_git_id(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn has_any_provider_identity(record: &PullRequestRecord) -> bool {
    record.provider_id.is_some()
        || record.provider_node_id.is_some()
        || record.number.is_some()
        || record.canonical_url.is_some()
        || record.base_revision.is_some()
        || record.head_revision.is_some()
        || record.observed_title.is_some()
        || record.observed_body_digest.is_some()
        || record.provider_created_at.is_some()
        || record.provider_updated_at.is_some()
}
