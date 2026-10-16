//! Atomic cancellation with a distinct replacement input.
use super::{CommandStatus, ResponseControlTarget};
use serde::{Deserialize, Serialize};

/// Retain the entire body and key when recovering an uncertain interruption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseInterruptionRequest {
    /// Active source turn observed through response controls.
    pub turn_sequence: u64,
    /// Agent aggregate revision from the same observation.
    pub expected_agent_revision: u64,
    /// New message text, with attachment references when applicable.
    pub text: String,
}

string_enum! { /// Accepted atomic interruption discriminator.
    pub enum ResponseInterruptionObject { Interruption => "cma.response.interruption" }
}

/// Cancellation and a new response admitted together. Delivery of cancellation
/// does not establish the old turn's outcome; the replacement starts after that
/// turn ends, ahead of ordinary queued inputs without deleting them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseInterruption {
    /// Document discriminator.
    pub object: ResponseInterruptionObject,
    /// Cancellation command receipt and current delivery status.
    pub control: CommandStatus,
    /// Distinct replacement message receipt and current execution status.
    pub response: CommandStatus,
    /// Immutable original response and turn selected for cancellation.
    pub target: ResponseControlTarget,
    /// Originally submitted aggregate revision, including on retry.
    pub expected_agent_revision: u64,
}
