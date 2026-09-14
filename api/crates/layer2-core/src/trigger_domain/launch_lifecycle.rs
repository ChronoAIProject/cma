use crate::sandbox::FirstMessageDelivery;
use crate::seconds_from_rfc3339;

use super::requirements::valid_digest;
use super::{
    Launch, LaunchBlockedReason, LaunchConfirmation, LaunchConsentSurface, LaunchEventKind,
    LaunchSeed, LaunchState, LaunchStep, LaunchStepKey, LaunchStepStatus, LaunchWorkState,
    LaunchWorkStep, RequirementsSnapshot, ResolvedResource, ReviewNeed, ReviewRequirements,
    TriggerError,
};

impl Launch {
    pub fn new(seed: LaunchSeed) -> Result<Self, TriggerError> {
        let first_message = if seed.has_first_message {
            FirstMessageDelivery::Queued
        } else {
            FirstMessageDelivery::Completed
        };
        let mut launch = Self {
            id: crate::LaunchId::generate(),
            trigger_id: seed.trigger_id,
            revision_digest: seed.revision_digest,
            public_id: seed.public_id,
            owner: seed.owner,
            authority: seed.authority,
            placement_id: seed.placement_id,
            referrer_origin: seed.referrer_origin,
            state: LaunchState::Draft,
            steps: initial_steps(&seed.created_at, first_message),
            resolution: seed.resolution,
            requirements: seed.requirements,
            review: seed.review,
            trust: seed.trust,
            first_message,
            first_message_evidence: None,
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
        launch.sync_requirement_step(&launch.created_at.clone())?;
        launch.validate()?;
        Ok(launch)
    }

    pub fn start(&mut self, confirmation: LaunchConfirmation) -> Result<(), TriggerError> {
        if self.state != LaunchState::Draft
            || !self.review.can_start_now
            || !self.confirmations.is_empty()
        {
            return Err(TriggerError::new("launch_not_startable", "state"));
        }
        confirmation.validate()?;
        let at = confirmation.confirmed_at.clone();
        self.confirmations.push(confirmation);
        self.begin_start(at)
    }

    pub fn confirm_landing_start(
        &mut self,
        confirmation: LaunchConfirmation,
    ) -> Result<(), TriggerError> {
        self.validate()?;
        confirmation.validate()?;
        if self.state != LaunchState::Draft
            || !self.confirmations.is_empty()
            || confirmation.surface != LaunchConsentSurface::Landing
            || confirmation.trust_confirmed_digest != self.trust.confirmation_digest
            || self.review.needs.iter().any(|need| {
                matches!(
                    need,
                    ReviewNeed::OwnerRequiresReview | ReviewNeed::RepeatChoice
                )
            })
        {
            return Err(TriggerError::new(
                "invalid_launch_confirmation",
                "confirmation",
            ));
        }
        self.review = ReviewRequirements::build(&self.requirements, None, false, false)?;
        self.updated_at = confirmation.confirmed_at.clone();
        self.confirmations.push(confirmation);
        self.validate()
    }

    pub fn start_confirmed(&mut self, at: String) -> Result<(), TriggerError> {
        if self.state != LaunchState::Draft
            || !self.review.can_start_now
            || !matches!(self.confirmations.as_slice(), [confirmation] if confirmation.surface == LaunchConsentSurface::Landing)
        {
            return Err(TriggerError::new("launch_not_startable", "state"));
        }
        self.begin_start(at)
    }

    fn begin_start(&mut self, at: String) -> Result<(), TriggerError> {
        self.transition(LaunchState::Resolving, at.clone())?;
        self.sync_requirement_step(&at)?;
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
        // Reuse preserves the conversation, but still needs live parent resources.
        self.start(confirmation)
    }

    pub fn refresh_review(
        &mut self,
        requirements: RequirementsSnapshot,
        review: ReviewRequirements,
        at: String,
    ) -> Result<Option<LaunchEventKind>, TriggerError> {
        if self.state != LaunchState::Draft || seconds_from_rfc3339(&at).is_none() {
            return Err(TriggerError::new("launch_not_reviewable", "state"));
        }
        requirements.validate()?;
        review.validate()?;
        self.requirements = requirements;
        self.review = review;
        let event = self.sync_requirement_step(&at)?;
        self.updated_at = at;
        self.bump_revision()?;
        self.validate()?;
        Ok(event)
    }

    pub(super) fn sync_requirement_step(
        &mut self,
        at: &str,
    ) -> Result<Option<LaunchEventKind>, TriggerError> {
        let missing = self
            .requirements
            .services
            .iter()
            .any(|service| service.needs_connection());
        let (status, detail) = if missing {
            (
                LaunchStepStatus::Blocked,
                "Connect required accounts to continue",
            )
        } else {
            (
                LaunchStepStatus::Completed,
                "Required accounts are connected",
            )
        };
        if self
            .steps
            .iter()
            .any(|step| step.key == LaunchStepKey::CheckingRequirements && step.status == status)
        {
            return Ok(None);
        }
        self.set_step(LaunchStepKey::CheckingRequirements, status, detail, at)
            .map(Some)
    }

    pub fn transition(&mut self, next: LaunchState, at: String) -> Result<(), TriggerError> {
        if !self.state.can_transition(next) || seconds_from_rfc3339(&at).is_none() {
            return Err(TriggerError::new("invalid_launch_transition", "state"));
        }
        self.state = next;
        self.updated_at = at.clone();
        self.bump_revision()?;
        if next == LaunchState::Ready {
            self.ready_at = Some(at);
            self.work
                .schedule(LaunchWorkStep::Finish, self.updated_at.clone());
        } else if matches!(next, LaunchState::Cancelled | LaunchState::FailedTerminal) {
            self.work.stop_preserving_effect(self.updated_at.clone());
        } else if next.is_terminal() {
            self.work
                .schedule(LaunchWorkStep::None, self.updated_at.clone());
        }
        Ok(())
    }

    pub fn is_worker_due(&self, now: &str) -> bool {
        !self.state.is_terminal()
            && self
                .blocked_reason
                .as_ref()
                .is_none_or(LaunchBlockedReason::is_wakeable)
            && self.work.is_due(now)
    }

    pub fn block(&mut self, reason: LaunchBlockedReason, at: String) -> Result<(), TriggerError> {
        if self.state.is_terminal() || seconds_from_rfc3339(&at).is_none() {
            return Err(TriggerError::new("invalid_launch_block", "blocked_reason"));
        }
        self.blocked_reason = Some(reason);
        self.updated_at = at;
        self.work.release_lease();
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
        Ok(())
    }

    pub(super) fn bump_revision(&mut self) -> Result<(), TriggerError> {
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
