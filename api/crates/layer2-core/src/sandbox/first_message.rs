use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::seconds_from_rfc3339;

use super::SandboxModelError;

pub const MAX_FIRST_MESSAGE_BYTES: usize = 256 * 1024;
const MAX_ORIGIN_KEY_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstMessageDelivery {
    Queued,
    DispatchFenced,
    WriteConfirmed,
    ExecutionObserved,
    Completed,
    Failed,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstMessageFailure {
    pub code: String,
    pub detail: String,
    pub retryable: bool,
    pub occurred_at: String,
}

impl FirstMessageFailure {
    pub fn new(
        code: impl Into<String>,
        detail: impl Into<String>,
        retryable: bool,
        occurred_at: impl Into<String>,
    ) -> Result<Self, SandboxModelError> {
        let failure = Self {
            code: code.into(),
            detail: detail.into(),
            retryable,
            occurred_at: occurred_at.into(),
        };
        failure.validate()?;
        Ok(failure)
    }

    fn validate(&self) -> Result<(), SandboxModelError> {
        if self.code.trim().is_empty()
            || self.code.len() > 128
            || self.detail.trim().is_empty()
            || self.detail.len() > 4 * 1024
            || seconds_from_rfc3339(&self.occurred_at).is_none()
        {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.first_message.failure",
            ));
        }
        Ok(())
    }
}

/// Recorded at admission; legacy messages without this field remain unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstMessagePurpose {
    Setup,
    Task,
}

pub struct FirstMessagePreparation {
    pub purpose: FirstMessagePurpose,
    pub desktop: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstMessageRecord {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub desktop: bool,
    #[serde(skip)]
    pub common_binding: Option<super::agent_io::TriggerFirstMessageBinding>,
    pub origin_key: String,
    pub text: String,
    pub payload_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<FirstMessagePurpose>,
    pub state: FirstMessageDelivery,
    pub attempt: u32,
    pub recovery_generation: u32,
    pub fence_sequence: u64,
    #[serde(default)]
    pub dispatch_generation: u64,
    pub queued_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_fenced_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_confirmed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_observed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<FirstMessageFailure>,
}

impl FirstMessageRecord {
    pub fn new(
        origin_key: impl Into<String>,
        text: impl Into<String>,
        queued_at: impl Into<String>,
    ) -> Result<Self, SandboxModelError> {
        let text = text.into();
        let record = Self {
            desktop: false,
            common_binding: None,
            origin_key: origin_key.into(),
            payload_sha256: hex_sha256(text.as_bytes()),
            purpose: Some(FirstMessagePurpose::Task),
            text,
            state: FirstMessageDelivery::Queued,
            attempt: 0,
            recovery_generation: 0,
            fence_sequence: 0,
            dispatch_generation: 0,
            queued_at: queued_at.into(),
            dispatch_fenced_at: None,
            write_confirmed_at: None,
            execution_observed_at: None,
            completed_at: None,
            failed_at: None,
            turn_sequence: None,
            failure: None,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), SandboxModelError> {
        if self.origin_key.trim().is_empty()
            || self.origin_key.len() > MAX_ORIGIN_KEY_BYTES
            || self.text.trim().is_empty()
            || self.text.len() > MAX_FIRST_MESSAGE_BYTES
            || self.payload_sha256 != hex_sha256(self.text.as_bytes())
            || seconds_from_rfc3339(&self.queued_at).is_none()
            || self
                .timestamps()
                .flatten()
                .any(|value| seconds_from_rfc3339(value).is_none())
            || self
                .failure
                .as_ref()
                .is_some_and(|value| value.validate().is_err())
        {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.first_message",
            ));
        }

        let fenced = self.attempt > 0
            && self.fence_sequence > 0
            && self.dispatch_generation > 0
            && self.dispatch_fenced_at.is_some()
            && self.turn_sequence.is_some_and(|value| value > 0);
        let no_terminal =
            self.completed_at.is_none() && self.failed_at.is_none() && self.failure.is_none();
        let shape_is_valid = match self.state {
            FirstMessageDelivery::Queued => {
                self.dispatch_generation == 0
                    && self.dispatch_fenced_at.is_none()
                    && self.write_confirmed_at.is_none()
                    && self.execution_observed_at.is_none()
                    && no_terminal
            }
            FirstMessageDelivery::DispatchFenced => {
                fenced
                    && self.write_confirmed_at.is_none()
                    && self.execution_observed_at.is_none()
                    && no_terminal
            }
            FirstMessageDelivery::WriteConfirmed => {
                fenced
                    && self.write_confirmed_at.is_some()
                    && self.execution_observed_at.is_none()
                    && no_terminal
            }
            FirstMessageDelivery::ExecutionObserved => {
                fenced && self.execution_observed_at.is_some() && no_terminal
            }
            FirstMessageDelivery::Completed => {
                fenced && self.completed_at.is_some() && self.failure.is_none()
            }
            FirstMessageDelivery::Failed => {
                (fenced || self.unattempted_runtime_loss() || self.unattempted_common_failure())
                    && self.failed_at.is_some()
                    && self.completed_at.is_none()
                    && self.failure.is_some()
            }
            FirstMessageDelivery::RecoveryRequired => {
                fenced
                    && self.failed_at.is_some()
                    && self.completed_at.is_none()
                    && self.failure.is_some()
            }
        };
        shape_is_valid.then_some(()).ok_or_else(|| {
            SandboxModelError::invalid_value("agent.conversation.first_message.state")
        })
    }

    pub fn fence_dispatch(
        &mut self,
        turn_sequence: u64,
        fence_sequence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.fence_dispatch_for(turn_sequence, fence_sequence, fence_sequence, at)
    }

    pub fn fence_dispatch_for(
        &mut self,
        turn_sequence: u64,
        fence_sequence: u64,
        dispatch_generation: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if self.state != FirstMessageDelivery::Queued
            || (self.common_binding.is_some()
                && (self.attempt != 0 || self.recovery_generation != 0))
            || turn_sequence == 0
            || fence_sequence == 0
            || dispatch_generation == 0
            || self
                .turn_sequence
                .is_some_and(|existing| existing != turn_sequence)
        {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.first_message.dispatch_fence",
            ));
        }
        let attempt = self
            .attempt
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("first_message.attempt"))?;
        self.state = FirstMessageDelivery::DispatchFenced;
        self.attempt = attempt;
        self.fence_sequence = fence_sequence;
        self.dispatch_generation = dispatch_generation;
        self.dispatch_fenced_at = Some(at);
        self.turn_sequence = Some(turn_sequence);
        self.validate()
    }

    pub fn mark_write_confirmed(&mut self, at: String) -> Result<(), SandboxModelError> {
        if self.common_binding.is_some() && self.state == FirstMessageDelivery::RecoveryRequired {
            self.failed_at = None;
            self.failure = None;
        } else {
            self.require_state(FirstMessageDelivery::DispatchFenced)?;
        }
        self.state = FirstMessageDelivery::WriteConfirmed;
        self.write_confirmed_at = Some(at);
        self.validate()
    }

    pub fn mark_execution_observed(&mut self, at: String) -> Result<(), SandboxModelError> {
        if !matches!(
            self.state,
            FirstMessageDelivery::DispatchFenced
                | FirstMessageDelivery::WriteConfirmed
                | FirstMessageDelivery::RecoveryRequired
        ) {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.first_message.execution",
            ));
        }
        self.failed_at = None;
        self.failure = None;
        self.state = FirstMessageDelivery::ExecutionObserved;
        self.execution_observed_at = Some(at);
        self.validate()
    }

    pub fn complete(&mut self, at: String) -> Result<(), SandboxModelError> {
        if !matches!(
            self.state,
            FirstMessageDelivery::DispatchFenced
                | FirstMessageDelivery::WriteConfirmed
                | FirstMessageDelivery::ExecutionObserved
                | FirstMessageDelivery::RecoveryRequired
        ) {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.first_message.completion",
            ));
        }
        self.failed_at = None;
        self.failure = None;
        self.state = FirstMessageDelivery::Completed;
        self.completed_at = Some(at);
        self.validate()
    }

    pub fn fail(&mut self, failure: FirstMessageFailure) -> Result<(), SandboxModelError> {
        if !matches!(
            self.state,
            FirstMessageDelivery::DispatchFenced
                | FirstMessageDelivery::WriteConfirmed
                | FirstMessageDelivery::ExecutionObserved
                | FirstMessageDelivery::RecoveryRequired
        ) {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.first_message.failure",
            ));
        }
        self.state = FirstMessageDelivery::Failed;
        self.failed_at = Some(failure.occurred_at.clone());
        self.failure = Some(failure);
        self.validate()
    }

    pub fn require_recovery(
        &mut self,
        failure: FirstMessageFailure,
    ) -> Result<(), SandboxModelError> {
        if !matches!(
            self.state,
            FirstMessageDelivery::DispatchFenced | FirstMessageDelivery::WriteConfirmed
        ) {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.first_message.recovery",
            ));
        }
        self.state = FirstMessageDelivery::RecoveryRequired;
        self.failed_at = Some(failure.occurred_at.clone());
        self.failure = Some(failure);
        self.validate()
    }

    pub fn retry_after_pre_write_failure(&mut self) -> Result<(), SandboxModelError> {
        if self.common_binding.is_some() {
            return Err(SandboxModelError::invalid_value(
                "first_message.common_retry",
            ));
        }
        self.require_state(FirstMessageDelivery::DispatchFenced)?;
        self.state = FirstMessageDelivery::Queued;
        self.dispatch_fenced_at = None;
        self.dispatch_generation = 0;
        self.validate()
    }

    pub fn authorize_recovery(
        &mut self,
        payload_sha256: &str,
        duplicate_execution_acknowledged: bool,
        queued_at: String,
    ) -> Result<(), SandboxModelError> {
        if self.state != FirstMessageDelivery::RecoveryRequired
            || self.common_binding.is_some()
            || payload_sha256 != self.payload_sha256
            || !duplicate_execution_acknowledged
        {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.first_message.recovery_authorization",
            ));
        }
        self.recovery_generation = self
            .recovery_generation
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("first_message.recovery_generation"))?;
        self.state = FirstMessageDelivery::Queued;
        self.queued_at = queued_at;
        self.dispatch_fenced_at = None;
        self.dispatch_generation = 0;
        self.write_confirmed_at = None;
        self.execution_observed_at = None;
        self.failed_at = None;
        self.turn_sequence = None;
        self.failure = None;
        self.validate()
    }

    pub const fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            FirstMessageDelivery::Completed | FirstMessageDelivery::Failed
        )
    }

    pub(crate) fn fail_runtime_loss(&mut self, at: String) -> Result<(), SandboxModelError> {
        if self.is_terminal() {
            return Ok(());
        }
        self.state = FirstMessageDelivery::Failed;
        self.failed_at = Some(at.clone());
        self.failure = Some(FirstMessageFailure::new(
            "runtime_lost",
            "the runtime was lost before the first message completed",
            false,
            at,
        )?);
        self.validate()
    }

    pub fn fail_common_before_dispatch(&mut self, at: String) -> Result<(), SandboxModelError> {
        if self.common_binding.is_none()
            || self.state != FirstMessageDelivery::Queued
            || self.attempt != 0
        {
            return Err(SandboxModelError::invalid_value(
                "first_message.common_failure",
            ));
        }
        self.state = FirstMessageDelivery::Failed;
        self.failed_at = Some(at.clone());
        self.failure = Some(FirstMessageFailure::new(
            "trigger_command_refused_before_dispatch",
            "the accepted Trigger command ended before its first prompt was dispatched",
            false,
            at,
        )?);
        self.validate()
    }
    fn unattempted_common_failure(&self) -> bool {
        self.attempt == 0
            && self.recovery_generation == 0
            && self.fence_sequence == 0
            && self.dispatch_generation == 0
            && self.turn_sequence.is_none()
            && self.dispatch_fenced_at.is_none()
            && self.write_confirmed_at.is_none()
            && self.execution_observed_at.is_none()
            && self.failure.as_ref().is_some_and(|failure| {
                failure.code == "trigger_command_refused_before_dispatch" && !failure.retryable
            })
    }

    fn unattempted_runtime_loss(&self) -> bool {
        self.dispatch_generation == 0
            && self.dispatch_fenced_at.is_none()
            && self.write_confirmed_at.is_none()
            && self.execution_observed_at.is_none()
            && self
                .failure
                .as_ref()
                .is_some_and(|failure| failure.code == "runtime_lost" && !failure.retryable)
    }

    fn require_state(&self, expected: FirstMessageDelivery) -> Result<(), SandboxModelError> {
        (self.state == expected).then_some(()).ok_or_else(|| {
            SandboxModelError::invalid_value("agent.conversation.first_message.state")
        })
    }

    fn timestamps(&self) -> impl Iterator<Item = Option<&str>> {
        [
            self.dispatch_fenced_at.as_deref(),
            self.write_confirmed_at.as_deref(),
            self.execution_observed_at.as_deref(),
            self.completed_at.as_deref(),
            self.failed_at.as_deref(),
        ]
        .into_iter()
    }
}

fn hex_sha256(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
