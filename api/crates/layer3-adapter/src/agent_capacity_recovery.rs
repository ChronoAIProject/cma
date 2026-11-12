//! Restricted retained-conversation recovery. No Layer 2 types or input payloads
//! cross this boundary. Only a native producer may supply the confirmation.
use serde::{Deserialize, Serialize};

use crate::{ConversationChannel, PromptIdentity, RuntimeHandle};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapacityRecoveryError {
    #[error("restricted native recovery is unsupported")]
    Unsupported,
    #[error("restricted native recovery is unavailable")]
    Unavailable,
    #[error("restricted recovery identity is invalid or changed")]
    IdentityConflict,
    #[error("native state requires input/output reconciliation")]
    StateUnconfirmed,
    #[error("retained output boundary is incomplete")]
    OutputUnconfirmed,
}

pub type CapacityRecoveryResult<T> = Result<T, CapacityRecoveryError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedAgentTarget {
    pub sandbox_id: String,
    pub agent_id: String,
    pub runtime: RuntimeHandle,
    pub boot_id: String,
    pub provision_generation: u64,
    pub home_identity: String,
    pub conversation_id: String,
    /// Independent of the provider allocation generation.
    pub conversation_generation: u64,
    pub original_open_operation: String,
    pub original_open_sha256: String,
    /// Already admitted turn, never input to submit again.
    pub active_turn: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityRecoveryRequest {
    pub operation_key: String,
    pub target: RetainedAgentTarget,
    pub writer_fence: u64,
    /// Fresh challenge for this attachment; an earlier observation cannot pass.
    pub challenge: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfirmedAgentState {
    Idle,
    Working {
        prompt_id: String,
    },
    WaitingApproval {
        prompt_id: String,
        approval_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedOutputBoundary {
    pub stream_id: String,
    pub acknowledged_through: u64,
    pub first_retained: u64,
    pub observed_through: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityRecoveryConfirmation {
    pub request: CapacityRecoveryRequest,
    pub native_state: ConfirmedAgentState,
    pub output: RetainedOutputBoundary,
}

impl CapacityRecoveryRequest {
    pub fn validate(&self) -> CapacityRecoveryResult<()> {
        let target = &self.target;
        if !identifier(&self.operation_key, 256)
            || !hex(&self.challenge, 32)
            || self.writer_fence == 0
            || self.writer_fence == u64::MAX
            || !identifier(&target.sandbox_id, 256)
            || !identifier(&target.agent_id, 256)
            || !identifier(target.runtime.as_str(), 512)
            || !hex(&target.boot_id, 32)
            || target.provision_generation == 0
            || !crate::valid_home_identity(&target.home_identity)
            || !identifier(&target.conversation_id, 512)
            || target.conversation_generation == 0
            || target.original_open_operation != format!("agent:{}:open:v1", target.agent_id)
            || !hex(&target.original_open_sha256, 64)
            || target
                .active_turn
                .is_some_and(|turn| turn == 0 || turn == u64::MAX)
        {
            return Err(CapacityRecoveryError::IdentityConflict);
        }
        Ok(())
    }
}

impl CapacityRecoveryConfirmation {
    pub fn validate_for(&self, request: &CapacityRecoveryRequest) -> CapacityRecoveryResult<()> {
        request.validate()?;
        if &self.request != request {
            return Err(CapacityRecoveryError::IdentityConflict);
        }
        let expected = request.target.active_turn.map(|turn| {
            PromptIdentity::new(
                &request.target.conversation_id,
                request.target.conversation_generation,
                turn,
            )
        });
        let observed = match &self.native_state {
            ConfirmedAgentState::Idle => None,
            ConfirmedAgentState::Working { prompt_id } => Some(prompt_id),
            ConfirmedAgentState::WaitingApproval {
                prompt_id,
                approval_id,
            } => {
                if !identifier(approval_id, 512) {
                    return Err(CapacityRecoveryError::StateUnconfirmed);
                }
                Some(prompt_id)
            }
        };
        if expected.as_ref().map(PromptIdentity::as_str) != observed.map(String::as_str) {
            // A terminal prompt in retained output must be durably settled first.
            // Never manufacture idle state by discarding the admitted turn.
            return Err(CapacityRecoveryError::StateUnconfirmed);
        }
        let output = &self.output;
        if !identifier(&output.stream_id, 256)
            || output.acknowledged_through.checked_add(1) != Some(output.first_retained)
            || output.observed_through < output.acknowledged_through
            || output.observed_through == u64::MAX
        {
            return Err(CapacityRecoveryError::OutputUnconfirmed);
        }
        Ok(())
    }
}

/// Constructed only after the producer's identity/state/output proof validates.
/// The CMA recovery adapter keeps this channel unpublished until durable commit.
pub struct CapacityRecoveryConnection {
    channel: ConversationChannel,
    confirmation: CapacityRecoveryConfirmation,
}

impl CapacityRecoveryConnection {
    pub(crate) fn verified(
        request: &CapacityRecoveryRequest,
        channel: ConversationChannel,
        confirmation: CapacityRecoveryConfirmation,
    ) -> CapacityRecoveryResult<Self> {
        confirmation.validate_for(request)?;
        if channel.conversation_id.as_deref() != Some(request.target.conversation_id.as_str()) {
            return Err(CapacityRecoveryError::IdentityConflict);
        }
        Ok(Self {
            channel,
            confirmation,
        })
    }

    pub fn confirmation(&self) -> &CapacityRecoveryConfirmation {
        &self.confirmation
    }

    /// Only the durable recovery consumer may pass this into staged registry
    /// activation. This operation itself sends no input or acknowledgement.
    pub fn into_parts(self) -> (ConversationChannel, CapacityRecoveryConfirmation) {
        (self.channel, self.confirmation)
    }
}

fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

#[cfg(test)]
pub(crate) mod tests;
