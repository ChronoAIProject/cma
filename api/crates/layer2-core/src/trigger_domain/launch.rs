use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, LaunchId, PlacementId, PublicTriggerId, SubjectId, TriggerId};
use crate::sandbox::FirstMessageDelivery;

use super::{
    LaunchResolution, LaunchTrustManifest, LaunchWorkState, RequirementsSnapshot,
    ReviewRequirements,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchState {
    Draft,
    Resolving,
    SandboxPending,
    WorkspacePending,
    AgentPending,
    MessagePending,
    Ready,
    Completed,
    FailedRetryable,
    FailedTerminal,
    Cancelled,
}

impl LaunchState {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::FailedTerminal | Self::Cancelled
        )
    }

    pub const fn can_cancel(self) -> bool {
        !matches!(
            self,
            Self::Ready | Self::Completed | Self::FailedTerminal | Self::Cancelled
        )
    }

    pub(super) fn can_transition(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::Resolving | Self::Cancelled)
                | (
                    Self::Resolving,
                    Self::SandboxPending
                        | Self::Ready
                        | Self::FailedRetryable
                        | Self::FailedTerminal
                        | Self::Cancelled
                )
                | (
                    Self::SandboxPending,
                    Self::WorkspacePending
                        | Self::FailedRetryable
                        | Self::FailedTerminal
                        | Self::Cancelled
                )
                | (
                    Self::WorkspacePending,
                    Self::AgentPending
                        | Self::FailedRetryable
                        | Self::FailedTerminal
                        | Self::Cancelled
                )
                | (
                    Self::AgentPending,
                    Self::MessagePending
                        | Self::Ready
                        | Self::FailedRetryable
                        | Self::FailedTerminal
                        | Self::Cancelled
                )
                | (
                    Self::MessagePending,
                    Self::Ready | Self::FailedRetryable | Self::FailedTerminal | Self::Cancelled
                )
                | (Self::Ready, Self::Completed)
                | (Self::FailedRetryable, Self::Resolving | Self::Cancelled)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchBlockedReason {
    ConnectionRequired { service: crate::ServiceId },
    OrchestrationPaused,
    SandboxPausedReviving,
    RecoveryRequired,
}

impl LaunchBlockedReason {
    pub const fn is_wakeable(&self) -> bool {
        matches!(
            self,
            Self::ConnectionRequired { .. }
                | Self::OrchestrationPaused
                | Self::SandboxPausedReviving
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchStepKey {
    SigningIn,
    CheckingRequirements,
    Sandbox,
    Workspace,
    Agent,
    FirstMessage,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchStepStatus {
    Pending,
    Running,
    Completed,
    Blocked,
    Retryable,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchStep {
    pub key: LaunchStepKey,
    pub status: LaunchStepStatus,
    pub safe_detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchFailure {
    pub code: String,
    pub safe_detail: String,
    pub retryable: bool,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchMessageEvidence {
    pub agent_id: AgentId,
    pub origin_key: String,
    pub payload_sha256: String,
    pub recovery_generation: u32,
    pub attempt: u32,
    pub fence_sequence: u64,
    pub dispatch_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchConsentSurface {
    Landing,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchAuthorityClass {
    Visitor,
    SandboxCreator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchConfirmation {
    pub surface: LaunchConsentSurface,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_confirmed_digest: Option<String>,
    #[serde(default)]
    pub acknowledgements: Vec<String>,
    pub confirmed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSeed {
    pub trigger_id: TriggerId,
    pub revision_digest: String,
    pub public_id: PublicTriggerId,
    pub owner: SubjectId,
    pub authority: LaunchAuthorityClass,
    pub placement_id: Option<PlacementId>,
    pub referrer_origin: Option<String>,
    pub requirements: RequirementsSnapshot,
    pub review: ReviewRequirements,
    pub trust: LaunchTrustManifest,
    pub resolution: LaunchResolution,
    pub has_first_message: bool,
    pub command_key_hash: String,
    pub command_fingerprint: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub id: LaunchId,
    pub trigger_id: TriggerId,
    pub revision_digest: String,
    pub public_id: PublicTriggerId,
    pub owner: SubjectId,
    pub authority: LaunchAuthorityClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_id: Option<PlacementId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referrer_origin: Option<String>,
    pub state: LaunchState,
    pub steps: Vec<LaunchStep>,
    pub resolution: LaunchResolution,
    pub requirements: RequirementsSnapshot,
    pub review: ReviewRequirements,
    pub trust: LaunchTrustManifest,
    pub first_message: FirstMessageDelivery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_message_evidence: Option<LaunchMessageEvidence>,
    #[serde(default)]
    pub confirmations: Vec<LaunchConfirmation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<LaunchBlockedReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<LaunchFailure>,
    pub work: LaunchWorkState,
    pub command_key_hash: String,
    pub command_fingerprint: String,
    pub event_head: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<String>,
    pub revision: u64,
}
