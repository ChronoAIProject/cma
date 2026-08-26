use serde::{Deserialize, Serialize};

use super::{
    Confirmation, ConfirmationAction, ContributionId, DiffArtifactId, LaunchId,
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
        let has_complete_objects =
            self.base_tree_id.is_some() && self.tree_id.is_some() && self.commit_id.is_some();
        let has_partial_objects = self.base_tree_id.is_some()
            || self.tree_id.is_some()
            || self.commit_id.is_some()
            || !self.object_ids.is_empty();
        match self.state {
            PublicationState::IntentRecorded
                if has_partial_objects || self.resulting_head.is_some() =>
            {
                return Err(TriggerValidationError::field(
                    "contribution.state",
                    "intent_recorded cannot claim Git object or ref evidence",
                ));
            }
            PublicationState::ObjectsWritten | PublicationState::RefUpdatePending
                if !has_complete_objects || self.resulting_head.is_some() =>
            {
                return Err(TriggerValidationError::field(
                    "contribution.state",
                    "prepared publication requires complete Git object evidence and no resulting ref",
                ));
            }
            PublicationState::Published
                if !has_complete_objects || self.resulting_head != self.commit_id =>
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestRecord {
    pub launch_id: LaunchId,
    pub marker: String,
    pub number: u64,
    pub canonical_url: String,
    pub state: PullRequestState,
    pub upstream_repository_id: u64,
    pub fork_repository_id: u64,
    pub base_ref: String,
    pub head_ref: String,
    pub base_revision: String,
    pub head_revision: String,
    pub title: String,
    pub body_digest: String,
    pub mergeability: Mergeability,
    pub checks: CheckSummaryState,
    pub confirmation: Confirmation,
    pub intent_key: String,
    pub created_at: String,
    pub updated_at: String,
}

impl PullRequestRecord {
    pub fn validate_intent(&self, launch: &LaunchId) -> Result<(), TriggerValidationError> {
        if &self.launch_id != launch
            || !matches!(
                self.confirmation.action,
                ConfirmationAction::OpenPullRequest | ConfirmationAction::UpdatePullRequest
            )
            || self.upstream_repository_id == 0
            || self.fork_repository_id == 0
            || self.base_ref.trim().is_empty()
            || self.head_ref.trim().is_empty()
            || self.marker.trim().is_empty()
            || self.intent_key.trim().is_empty()
        {
            return Err(TriggerValidationError::field(
                "pull_request",
                "is not a valid explicit pull-request intent for this Launch",
            ));
        }
        Ok(())
    }
}
