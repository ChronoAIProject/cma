use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::seconds_from_rfc3339;

use super::SandboxModelError;

pub const MAX_QUEUED_AGENT_MESSAGES: usize = 64;
pub const MAX_AGENT_MESSAGE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueuedMessageState {
    Queued,
    DispatchFenced,
    Delivered,
    RecoveryRequired,
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
}
