use crate::sandbox::FirstMessageDelivery;
use crate::seconds_from_rfc3339;

use super::launch_message_evidence::validate_persisted_message_evidence;
use super::requirements::valid_digest;
use super::{
    Launch, LaunchBlockedReason, LaunchEffectState, LaunchFailure, LaunchState, LaunchStep,
    LaunchStepKey, LaunchStepStatus, TriggerError,
};

const MAX_SAFE_DETAIL_BYTES: usize = 512;
const MAX_FAILURE_CODE_BYTES: usize = 128;
const STEP_KEYS: [LaunchStepKey; 7] = [
    LaunchStepKey::SigningIn,
    LaunchStepKey::CheckingRequirements,
    LaunchStepKey::Sandbox,
    LaunchStepKey::Workspace,
    LaunchStepKey::Agent,
    LaunchStepKey::FirstMessage,
    LaunchStepKey::Ready,
];

impl Launch {
    pub fn validate(&self) -> Result<(), TriggerError> {
        let created = seconds_from_rfc3339(&self.created_at);
        let updated = seconds_from_rfc3339(&self.updated_at);
        if !valid_digest(&self.revision_digest)
            || !valid_digest(&self.command_key_hash)
            || !valid_digest(&self.command_fingerprint)
            || self.revision == 0
            || created.is_none()
            || updated.is_none()
            || updated < created
        {
            return Err(TriggerError::new("invalid_launch", "launch"));
        }
        let created = created.expect("checked");
        let updated = updated.expect("checked");
        validate_optional_time(self.started_at.as_deref(), created, updated, "started_at")?;
        validate_optional_time(self.ready_at.as_deref(), created, updated, "ready_at")?;
        validate_referrer(self.referrer_origin.as_deref())?;
        validate_steps(&self.steps, created, updated)?;
        validate_failure(self)?;
        validate_block(self)?;
        self.requirements.validate()?;
        self.review
            .validate_for_launch(&self.requirements, self.started_at.is_none())?;
        self.trust.validate()?;
        self.resolution.validate()?;
        validate_persisted_message_evidence(self).map_err(|_| {
            TriggerError::new("invalid_launch_first_message", "first_message_evidence")
        })?;
        self.work
            .validate(&self.id, self.state == LaunchState::Cancelled)?;
        for confirmation in &self.confirmations {
            confirmation.validate()?;
            validate_optional_time(
                Some(&confirmation.confirmed_at),
                created,
                updated,
                "confirmations.confirmed_at",
            )?;
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
        {
            return Err(TriggerError::new("invalid_launch", "state"));
        }
        validate_first_message_state(self)?;
        Ok(())
    }
}

fn validate_steps(steps: &[LaunchStep], created: u64, updated: u64) -> Result<(), TriggerError> {
    if steps.len() != STEP_KEYS.len() || !steps.iter().map(|step| step.key).eq(STEP_KEYS) {
        return Err(TriggerError::new("invalid_launch_steps", "steps"));
    }
    steps
        .iter()
        .try_for_each(|step| validate_step(step, created, updated))
}

fn validate_step(step: &LaunchStep, created: u64, updated: u64) -> Result<(), TriggerError> {
    if !valid_safe_detail(&step.safe_detail) {
        return Err(TriggerError::new(
            "invalid_launch_step",
            "steps.safe_detail",
        ));
    }
    let started = optional_time(step.started_at.as_deref(), created, updated);
    let completed = optional_time(step.completed_at.as_deref(), created, updated);
    let valid_shape = match step.status {
        LaunchStepStatus::Pending => started == Some(None) && completed == Some(None),
        LaunchStepStatus::Completed => {
            matches!((started, completed), (Some(Some(a)), Some(Some(b))) if b >= a)
        }
        LaunchStepStatus::Running
        | LaunchStepStatus::Blocked
        | LaunchStepStatus::Retryable
        | LaunchStepStatus::Failed => {
            started.is_some_and(|value| value.is_some()) && completed == Some(None)
        }
    };
    valid_shape
        .then_some(())
        .ok_or_else(|| TriggerError::new("invalid_launch_step", "steps.timestamps"))
}

fn validate_failure(launch: &Launch) -> Result<(), TriggerError> {
    match (launch.state, launch.failure.as_ref()) {
        (LaunchState::FailedRetryable, Some(failure)) if failure.retryable => {
            failure.validate(&launch.created_at, &launch.updated_at)
        }
        (LaunchState::FailedTerminal, Some(failure)) if !failure.retryable => {
            failure.validate(&launch.created_at, &launch.updated_at)
        }
        (LaunchState::FailedRetryable | LaunchState::FailedTerminal, _) | (_, Some(_)) => {
            Err(TriggerError::new("invalid_launch_failure", "failure"))
        }
        (_, None) => Ok(()),
    }
}

impl LaunchFailure {
    fn validate(&self, created_at: &str, updated_at: &str) -> Result<(), TriggerError> {
        let created = seconds_from_rfc3339(created_at).expect("Launch timestamps were checked");
        let updated = seconds_from_rfc3339(updated_at).expect("Launch timestamps were checked");
        if self.code.trim().is_empty()
            || self.code.len() > MAX_FAILURE_CODE_BYTES
            || self.code.chars().any(char::is_control)
            || self.safe_detail.trim().is_empty()
            || !valid_safe_detail(&self.safe_detail)
            || optional_time(Some(&self.occurred_at), created, updated).is_none()
        {
            return Err(TriggerError::new("invalid_launch_failure", "failure"));
        }
        Ok(())
    }
}

fn validate_block(launch: &Launch) -> Result<(), TriggerError> {
    let recovery_evidence = launch.first_message == FirstMessageDelivery::RecoveryRequired
        || launch
            .work
            .effect
            .as_ref()
            .is_some_and(|effect| effect.state == LaunchEffectState::RecoveryRequired);
    if launch.blocked_reason.is_some() && launch.work.lease.is_some()
        || matches!(
            launch.blocked_reason,
            Some(LaunchBlockedReason::RecoveryRequired)
        ) && !recovery_evidence
        || launch.blocked_reason.is_some()
            && !matches!(
                launch.blocked_reason,
                Some(LaunchBlockedReason::RecoveryRequired)
            )
            && recovery_evidence
    {
        return Err(TriggerError::new("invalid_launch_block", "blocked_reason"));
    }
    Ok(())
}

fn validate_first_message_state(launch: &Launch) -> Result<(), TriggerError> {
    let step = launch
        .steps
        .iter()
        .find(|step| step.key == LaunchStepKey::FirstMessage)
        .expect("Launch step keys were checked");
    let message_work = matches!(
        launch.work.step,
        super::LaunchWorkStep::DeliverFirstMessage | super::LaunchWorkStep::WaitForFirstMessage
    );
    let valid = match launch.first_message {
        FirstMessageDelivery::Queued => {
            if launch.state == LaunchState::MessagePending {
                launch.work.step == super::LaunchWorkStep::DeliverFirstMessage
                    && matches!(
                        step.status,
                        LaunchStepStatus::Running | LaunchStepStatus::Retryable
                    )
            } else {
                !matches!(launch.state, LaunchState::Ready | LaunchState::Completed)
            }
        }
        FirstMessageDelivery::DispatchFenced => {
            launch.state == LaunchState::Cancelled
                || launch.state == LaunchState::MessagePending
                    && launch.work.step == super::LaunchWorkStep::WaitForFirstMessage
                    && step.status == LaunchStepStatus::Running
        }
        FirstMessageDelivery::WriteConfirmed
        | FirstMessageDelivery::ExecutionObserved
        | FirstMessageDelivery::Completed => {
            if matches!(launch.state, LaunchState::Ready | LaunchState::Completed) {
                step.status == LaunchStepStatus::Completed
            } else if launch.state == LaunchState::MessagePending {
                launch.work.step == super::LaunchWorkStep::Finish
                    && step.status == LaunchStepStatus::Completed
            } else {
                launch.state == LaunchState::Cancelled
                    || launch.first_message == FirstMessageDelivery::Completed
                        && launch.first_message_evidence.is_none()
            }
        }
        FirstMessageDelivery::RecoveryRequired => {
            launch.state == LaunchState::Cancelled
                || launch.state == LaunchState::MessagePending
                    && launch.blocked_reason == Some(LaunchBlockedReason::RecoveryRequired)
                    && launch.work.lease.is_none()
                    && message_work
                    && step.status == LaunchStepStatus::Blocked
        }
        FirstMessageDelivery::Failed => {
            launch.state == LaunchState::FailedTerminal
                && launch
                    .failure
                    .as_ref()
                    .is_some_and(|failure| !failure.retryable)
                && launch.blocked_reason.is_none()
                && step.status == LaunchStepStatus::Failed
        }
    };
    valid
        .then_some(())
        .ok_or_else(|| TriggerError::new("invalid_launch_first_message", "first_message"))
}

fn validate_optional_time(
    value: Option<&str>,
    minimum: u64,
    maximum: u64,
    field: &'static str,
) -> Result<(), TriggerError> {
    if optional_time(value, minimum, maximum).is_none() {
        Err(TriggerError::new("invalid_launch", field))
    } else {
        Ok(())
    }
}

fn optional_time(value: Option<&str>, minimum: u64, maximum: u64) -> Option<Option<u64>> {
    value
        .map(|value| {
            seconds_from_rfc3339(value).filter(|value| *value >= minimum && *value <= maximum)
        })
        .map_or(Some(None), |value| value.map(Some))
}

fn valid_safe_detail(value: &str) -> bool {
    value.len() <= MAX_SAFE_DETAIL_BYTES && !value.chars().any(char::is_control)
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
