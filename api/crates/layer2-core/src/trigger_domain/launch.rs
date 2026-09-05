use serde::{Deserialize, Serialize};

use crate::ids::{LaunchId, PlacementId, PublicTriggerId, SubjectId, TriggerId};
use crate::sandbox::FirstMessageDelivery;
use crate::seconds_from_rfc3339;

use super::requirements::valid_digest;
use super::{
    LaunchResolution, LaunchTrustManifest, LaunchWorkState, LaunchWorkStep, RequirementsSnapshot,
    ResolvedResource, ReviewRequirements, TriggerError,
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

    fn can_transition(self, next: Self) -> bool {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchConsentSurface {
    Landing,
    Review,
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

impl Launch {
    pub fn new(seed: LaunchSeed) -> Result<Self, TriggerError> {
        let first_message = if seed.has_first_message {
            FirstMessageDelivery::Queued
        } else {
            FirstMessageDelivery::Completed
        };
        let launch = Self {
            id: LaunchId::generate(),
            trigger_id: seed.trigger_id,
            revision_digest: seed.revision_digest,
            public_id: seed.public_id,
            owner: seed.owner,
            placement_id: seed.placement_id,
            referrer_origin: seed.referrer_origin,
            state: LaunchState::Draft,
            steps: initial_steps(&seed.created_at, first_message),
            resolution: seed.resolution,
            requirements: seed.requirements,
            review: seed.review,
            trust: seed.trust,
            first_message,
            confirmations: Vec::new(),
            blocked_reason: None,
            failure: None,
            work: LaunchWorkState::new(LaunchWorkStep::None, seed.created_at.clone()),
            command_key_hash: seed.command_key_hash,
            command_fingerprint: seed.command_fingerprint,
            event_head: 0,
            created_at: seed.created_at.clone(),
            updated_at: seed.created_at,
            started_at: None,
            ready_at: None,
            revision: 1,
        };
        launch.validate()?;
        Ok(launch)
    }

    pub fn start(&mut self, confirmation: LaunchConfirmation) -> Result<(), TriggerError> {
        if self.state != LaunchState::Draft || !self.review.can_start_now {
            return Err(TriggerError::new("launch_not_startable", "state"));
        }
        confirmation.validate()?;
        let at = confirmation.confirmed_at.clone();
        self.confirmations.push(confirmation);
        self.transition(LaunchState::Resolving, at.clone())?;
        self.started_at = Some(at.clone());
        self.work = LaunchWorkState::new(LaunchWorkStep::ResolveSandbox, at);
        self.validate()
    }

    pub fn start_existing(&mut self, confirmation: LaunchConfirmation) -> Result<(), TriggerError> {
        confirmation.validate()?;
        let at = confirmation.confirmed_at.clone();
        if self.state != LaunchState::Draft
            || !self.review.can_start_now
            || self.resolution.sandbox.is_pending()
            || self.resolution.workspace.is_pending()
            || !matches!(self.resolution.agent, ResolvedResource::Reused(_))
            || seconds_from_rfc3339(&at).is_none()
        {
            return Err(TriggerError::new(
                "invalid_existing_launch_resolution",
                "resolution",
            ));
        }
        self.confirmations.push(confirmation);
        self.state = LaunchState::Ready;
        self.first_message = FirstMessageDelivery::Completed;
        self.started_at = Some(at.clone());
        self.ready_at = Some(at.clone());
        self.updated_at = at.clone();
        for step in &mut self.steps {
            step.status = LaunchStepStatus::Completed;
            step.started_at.get_or_insert_with(|| at.clone());
            step.completed_at.get_or_insert_with(|| at.clone());
        }
        self.work = LaunchWorkState::new(LaunchWorkStep::None, at);
        self.bump_revision()?;
        self.validate()
    }

    pub fn refresh_review(
        &mut self,
        requirements: RequirementsSnapshot,
        review: ReviewRequirements,
        at: String,
    ) -> Result<(), TriggerError> {
        if self.state != LaunchState::Draft || seconds_from_rfc3339(&at).is_none() {
            return Err(TriggerError::new("launch_not_reviewable", "state"));
        }
        requirements.validate()?;
        review.validate()?;
        self.requirements = requirements;
        self.review = review;
        self.updated_at = at;
        self.bump_revision()?;
        self.validate()
    }

    pub fn transition(&mut self, next: LaunchState, at: String) -> Result<(), TriggerError> {
        if !self.state.can_transition(next) || seconds_from_rfc3339(&at).is_none() {
            return Err(TriggerError::new("invalid_launch_transition", "state"));
        }
        self.state = next;
        self.updated_at = at.clone();
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| TriggerError::new("launch_revision_overflow", "revision"))?;
        if next == LaunchState::Ready {
            self.ready_at = Some(at);
            self.work = LaunchWorkState::new(LaunchWorkStep::None, self.updated_at.clone());
        } else if next.is_terminal() {
            self.work = LaunchWorkState::new(LaunchWorkStep::None, self.updated_at.clone());
        }
        Ok(())
    }

    pub fn block(&mut self, reason: LaunchBlockedReason, at: String) -> Result<(), TriggerError> {
        if self.state.is_terminal() || seconds_from_rfc3339(&at).is_none() {
            return Err(TriggerError::new("invalid_launch_block", "blocked_reason"));
        }
        self.blocked_reason = Some(reason);
        self.updated_at = at;
        self.bump_revision()
    }

    pub fn resume(&mut self, at: String) -> Result<LaunchBlockedReason, TriggerError> {
        if seconds_from_rfc3339(&at).is_none() {
            return Err(TriggerError::new("invalid_timestamp", "updated_at"));
        }
        let reason = self
            .blocked_reason
            .take()
            .ok_or_else(|| TriggerError::new("launch_not_blocked", "blocked_reason"))?;
        self.updated_at = at;
        self.bump_revision()?;
        Ok(reason)
    }

    pub fn cancel(&mut self, at: String) -> Result<(), TriggerError> {
        if !self.state.can_cancel() {
            return Err(TriggerError::new("launch_not_cancellable", "state"));
        }
        self.transition(LaunchState::Cancelled, at)?;
        self.blocked_reason = None;
        self.work.step = LaunchWorkStep::None;
        self.work.lease = None;
        self.work.effect = None;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), TriggerError> {
        if !valid_digest(&self.revision_digest)
            || !valid_digest(&self.command_key_hash)
            || !valid_digest(&self.command_fingerprint)
            || self.revision == 0
            || seconds_from_rfc3339(&self.created_at).is_none()
            || seconds_from_rfc3339(&self.updated_at).is_none()
        {
            return Err(TriggerError::new("invalid_launch", "launch"));
        }
        validate_referrer(self.referrer_origin.as_deref())?;
        self.requirements.validate()?;
        self.review
            .validate_for_launch(&self.requirements, self.started_at.is_none())?;
        self.trust.validate()?;
        self.resolution.validate()?;
        for confirmation in &self.confirmations {
            confirmation.validate()?;
        }
        let requires_start = !matches!(self.state, LaunchState::Draft | LaunchState::Cancelled);
        let was_ready = matches!(self.state, LaunchState::Ready | LaunchState::Completed);
        if (self.state == LaunchState::Draft && self.started_at.is_some())
            || (requires_start && self.started_at.is_none())
            || (self.started_at.is_some() == self.confirmations.is_empty())
            || self.confirmations.len() > 1
            || (self.started_at.is_none()
                && self.review.trust_digest() != self.trust.confirmation_digest.as_deref())
            || self.confirmations.iter().any(|confirmation| {
                confirmation.trust_confirmed_digest.as_deref()
                    != self.trust.confirmation_digest.as_deref()
            })
            || (was_ready != self.ready_at.is_some())
            || self.state.is_terminal() && self.work.step.is_pending()
            || self.blocked_reason.is_some() && self.state.is_terminal()
            || self.steps.len() != 7
        {
            return Err(TriggerError::new("invalid_launch", "state"));
        }
        Ok(())
    }

    fn bump_revision(&mut self) -> Result<(), TriggerError> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| TriggerError::new("launch_revision_overflow", "revision"))?;
        Ok(())
    }
}

impl LaunchConfirmation {
    pub fn validate(&self) -> Result<(), TriggerError> {
        if seconds_from_rfc3339(&self.confirmed_at).is_none()
            || self
                .trust_confirmed_digest
                .as_deref()
                .is_some_and(|digest| !valid_digest(digest))
            || self.acknowledgements.len() > 32
            || self
                .acknowledgements
                .iter()
                .any(|value| value.trim().is_empty() || value.len() > 128)
        {
            return Err(TriggerError::new(
                "invalid_launch_confirmation",
                "confirmation",
            ));
        }
        Ok(())
    }
}

fn initial_steps(at: &str, first_message: FirstMessageDelivery) -> Vec<LaunchStep> {
    [
        (LaunchStepKey::SigningIn, LaunchStepStatus::Completed),
        (
            LaunchStepKey::CheckingRequirements,
            LaunchStepStatus::Completed,
        ),
        (LaunchStepKey::Sandbox, LaunchStepStatus::Pending),
        (LaunchStepKey::Workspace, LaunchStepStatus::Pending),
        (LaunchStepKey::Agent, LaunchStepStatus::Pending),
        (
            LaunchStepKey::FirstMessage,
            if first_message == FirstMessageDelivery::Completed {
                LaunchStepStatus::Completed
            } else {
                LaunchStepStatus::Pending
            },
        ),
        (LaunchStepKey::Ready, LaunchStepStatus::Pending),
    ]
    .into_iter()
    .map(|(key, status)| LaunchStep {
        key,
        status,
        safe_detail: String::new(),
        started_at: matches!(status, LaunchStepStatus::Completed).then(|| at.to_string()),
        completed_at: matches!(status, LaunchStepStatus::Completed).then(|| at.to_string()),
    })
    .collect()
}

fn validate_referrer(value: Option<&str>) -> Result<(), TriggerError> {
    let Some(value) = value else {
        return Ok(());
    };
    let url = url::Url::parse(value)
        .map_err(|_| TriggerError::new("invalid_referrer_origin", "referrer_origin"))?;
    if value.len() > 512
        || !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(TriggerError::new(
            "invalid_referrer_origin",
            "referrer_origin",
        ));
    }
    Ok(())
}
