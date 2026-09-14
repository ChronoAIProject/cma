use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::seconds_from_rfc3339;

use super::SandboxModelError;

pub const MAX_QUEUED_AGENT_MESSAGES: usize = 64;
pub const MAX_AGENT_MESSAGE_BYTES: usize = 256 * 1024;
pub const QUEUED_MESSAGE_DISPATCH_GRACE_SECONDS: u64 = 15 * 60;
pub(crate) const QUEUED_MESSAGE_RECOVERY_CODE: &str = "dispatch_settlement_missing";
pub(crate) const QUEUED_MESSAGE_RECOVERY_DETAIL: &str =
    "the queued-message write outcome was not settled before the dispatch grace elapsed";
pub(crate) const QUEUED_MESSAGE_RECOVERY_EVENT_CLASS: &str = "retryable_transient";
const QUEUED_MESSAGE_ORIGIN_PREFIX: &str = "queued-message:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueuedMessageState {
    Queued,
    DispatchFenced,
    Delivered,
    RecoveryRequired,
}

pub(super) fn queued_message_origin(sequence: u64) -> String {
    format!("{QUEUED_MESSAGE_ORIGIN_PREFIX}{sequence}")
}

pub(super) fn queued_message_origin_sequence(origin: &str) -> Option<u64> {
    origin
        .strip_prefix(QUEUED_MESSAGE_ORIGIN_PREFIX)?
        .parse()
        .ok()
        .filter(|sequence| *sequence > 0)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedAgentMessage {
    pub sequence: u64,
    pub content: String,
    pub payload_sha256: String,
    pub admitted_at: String,
    pub state: QueuedMessageState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_fenced_at: Option<String>,
}

fn hex_sha256(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

impl QueuedAgentMessage {
    pub fn new(
        sequence: u64,
        content: String,
        admitted_at: String,
    ) -> Result<Self, SandboxModelError> {
        let message = Self {
            sequence,
            payload_sha256: hex_sha256(content.as_bytes()),
            content,
            admitted_at,
            state: QueuedMessageState::Queued,
            dispatch_fenced_at: None,
        };
        message.validate()?;
        Ok(message)
    }

    pub fn validate(&self) -> Result<(), SandboxModelError> {
        let fence_shape = match self.state {
            QueuedMessageState::Queued => self.dispatch_fenced_at.is_none(),
            QueuedMessageState::DispatchFenced
            | QueuedMessageState::Delivered
            | QueuedMessageState::RecoveryRequired => self.dispatch_fenced_at.is_some(),
        };
        if self.sequence == 0
            || self.content.trim().is_empty()
            || self.content.len() > MAX_AGENT_MESSAGE_BYTES
            || self.payload_sha256 != hex_sha256(self.content.as_bytes())
            || seconds_from_rfc3339(&self.admitted_at).is_none()
            || self
                .dispatch_fenced_at
                .as_deref()
                .is_some_and(|value| seconds_from_rfc3339(value).is_none())
            || !fence_shape
        {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.queued_messages",
            ));
        }
        Ok(())
    }

    pub fn fence_dispatch(&mut self, at: String) -> Result<(), SandboxModelError> {
        if self.state != QueuedMessageState::Queued {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.queued_messages.state",
            ));
        }
        self.state = QueuedMessageState::DispatchFenced;
        self.dispatch_fenced_at = Some(at);
        self.validate()
    }

    pub fn mark_delivered(&mut self) -> Result<(), SandboxModelError> {
        if !matches!(
            self.state,
            QueuedMessageState::DispatchFenced | QueuedMessageState::RecoveryRequired
        ) {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.queued_messages.state",
            ));
        }
        self.state = QueuedMessageState::Delivered;
        self.validate()
    }

    pub fn mark_execution_observed(&mut self) -> Result<(), SandboxModelError> {
        if !matches!(
            self.state,
            QueuedMessageState::DispatchFenced | QueuedMessageState::RecoveryRequired
        ) {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.queued_messages.state",
            ));
        }
        self.state = QueuedMessageState::Delivered;
        self.validate()
    }

    pub fn require_recovery(&mut self) -> Result<(), SandboxModelError> {
        if self.state != QueuedMessageState::DispatchFenced {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.queued_messages.state",
            ));
        }
        self.state = QueuedMessageState::RecoveryRequired;
        self.validate()
    }

    pub fn retry_after_pre_write_failure(&mut self) -> Result<(), SandboxModelError> {
        if self.state != QueuedMessageState::DispatchFenced {
            return Err(SandboxModelError::invalid_value(
                "agent.conversation.queued_messages.state",
            ));
        }
        self.state = QueuedMessageState::Queued;
        self.dispatch_fenced_at = None;
        self.validate()
    }
}
