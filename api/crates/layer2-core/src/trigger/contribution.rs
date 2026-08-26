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
    pub state: PublicationState,
    pub expected_head: String,
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
            || self.expected_head.trim().is_empty()
            || self.intent_key.trim().is_empty()
        {
            return Err(TriggerValidationError::field(
                "contribution",
                "is not a valid publication intent for this Launch",
            ));
        }
        self.author.validate()?;
        self.committer.validate()
    }
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
