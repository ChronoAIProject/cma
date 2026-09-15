use layer2_core::sandbox::agent_io::{QueuedCancellationObservation, QueuedCancellationStatusView};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::super::controls::shapes::V2CommandStatus;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct V2QueuedCancellationRequest {
    pub source_command_id: String,
    #[schema(minimum = 1, maximum = 9223372036854775807_u64)]
    pub expected_command_revision: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct V2QueuedCancellationObservation {
    pub object: &'static str,
    pub response: V2CommandStatus,
    pub command_revision: u64,
    pub cancellable: bool,
}

impl From<QueuedCancellationObservation> for V2QueuedCancellationObservation {
    fn from(value: QueuedCancellationObservation) -> Self {
        Self {
            object: "cma.response.queued_cancellation.observation",
            response: value.response.into(),
            command_revision: value.command_revision,
            cancellable: value.cancellable,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct V2QueuedCancellationTarget {
    pub response_id: String,
    pub source_command_id: String,
    pub expected_command_revision: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct V2QueuedCancellation {
    pub object: &'static str,
    pub cancellation: V2CommandStatus,
    pub response: V2CommandStatus,
    pub target: V2QueuedCancellationTarget,
}

impl From<QueuedCancellationStatusView> for V2QueuedCancellation {
    fn from(value: QueuedCancellationStatusView) -> Self {
        Self {
            object: "cma.response.queued_cancellation",
            cancellation: value.cancellation.into(),
            response: value.response.into(),
            target: V2QueuedCancellationTarget {
                response_id: value.target.response_id.to_string(),
                source_command_id: value.target.source_command_id.to_string(),
                expected_command_revision: value.target.expected_command_revision,
            },
        }
    }
}
