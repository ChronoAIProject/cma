use super::{CommandStatus, ControlObserveMethod, ControlSubmitMethod};
use serde::{Deserialize, Serialize};

/// Immutable source selection. Persist the route, body and key before sending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedCancellationRequest {
    /// Exact source message command from the current observation.
    pub source_command_id: String,
    /// Original source command revision, distinct from the Agent revision.
    pub expected_command_revision: u64,
}
string_enum! { /// Queued cancellation observation discriminator.
    pub enum QueuedCancellationObservationObject { Observation => "cma.response.queued_cancellation.observation" }
}
/// Current source eligibility under fresh human/API operation authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedCancellationObservation {
    /// Document discriminator.
    pub object: QueuedCancellationObservationObject,
    /// Current source message status and receipt.
    pub response: CommandStatus,
    /// Current source command revision to retain in the intent.
    pub command_revision: u64,
    /// Observation only; the service rechecks authority and source state atomically.
    pub cancellable: bool,
}
/// Original source selection retained through every receipt recovery.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedCancellationTarget {
    /// Exact response identity.
    pub response_id: String,
    /// Original source command identity.
    pub source_command_id: String,
    /// Originally submitted source command revision.
    pub expected_command_revision: u64,
}
string_enum! { /// Committed queued cancellation discriminator.
    pub enum QueuedCancellationObject { Cancellation => "cma.response.queued_cancellation" }
}
/// Committed cancellation receipt with the source command's current safe status.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedCancellation {
    /// Document discriminator.
    pub object: QueuedCancellationObject,
    /// Completed cancellation command without a response ID, turn or delivery.
    pub cancellation: CommandStatus,
    /// Current safe source command view, resolved under fresh authority on replay.
    pub response: CommandStatus,
    /// Immutable original source selection.
    pub target: QueuedCancellationTarget,
}
string_enum! { /// Retained queued cancellation key header.
    pub enum QueuedCancellationKeyHeader { IdempotencyKey => "Idempotency-Key" }
}
string_enum! { /// Queued cancellation target semantics.
    pub enum QueuedCancellationTargetKind { UnboundResponseCommand => "unbound_response_command" }
}
string_enum! { /// Queued cancellation completion semantics.
    pub enum QueuedCancellationCompletionKind { SourceCancelled => "source_cancelled" }
}
/// Optional queued cancellation extension on deployments that support it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedCancellationCapabilities {
    /// Agent and response scoped endpoint template.
    pub path: String,
    /// Observation HTTP method.
    pub observe: ControlObserveMethod,
    /// Submission HTTP method.
    pub submit: ControlSubmitMethod,
    /// Required retained key header.
    pub idempotency_header: QueuedCancellationKeyHeader,
    /// Always true for this contract.
    #[serde(deserialize_with = "required_true")]
    pub idempotency_required: bool,
    /// Source selection semantics.
    pub target: QueuedCancellationTargetKind,
    /// Meaning of successful completion.
    pub completion: QueuedCancellationCompletionKind,
}
fn required_true<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = bool::deserialize(deserializer)?;
    if !value {
        return Err(serde::de::Error::custom(
            "queued cancellation requires idempotency",
        ));
    }
    Ok(value)
}
