//! Only finite admission views cross the hop. Replay and streaming stay on ingress.
use super::errors::{AdmissionFailure, ControlFailure, QueueFailure};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    sandbox::{agent_io::*, AgentOperationDelivery},
    AgentId,
};
use serde::{Deserialize, Serialize};

// A queued edit echoes the complete legal text once. JSON can encode each
// input byte as six bytes (\u00XX). The remaining receipt fields are IDs,
// timestamps, numeric/enumerated statuses and framing; reserve 64 KiB for them.
// Codec tests exercise maximal IDs and text escaping against this same bound.
pub(super) const MAX_PEER_REPLY_BYTES: usize =
    6 * layer2_core::sandbox::MAX_AGENT_MESSAGE_BYTES + 64 * 1024;

#[derive(Serialize, Deserialize)]
pub(super) enum Reply {
    Admit(Result<Status, AdmissionFailure>),
    TargetedControl(Result<ControlStatus, ControlFailure>),
    DirectTargetedControl(Result<ControlStatus, AdmissionFailure>),
    InterruptingMessage(Result<InterruptStatus, ControlFailure>),
    QueuedInput(Result<EditStatus, QueueFailure>),
    QueuedCancellation(Result<CancelStatus, QueueFailure>),
}
#[derive(Serialize, Deserialize)]
pub(super) struct Status(#[serde(with = "StatusFields")] CommandStatusView);
#[derive(Serialize, Deserialize)]
#[serde(remote = "CommandStatusView")]
struct StatusFields {
    #[serde(with = "ReceiptFields")]
    receipt: CommandReceiptView,
    status: AgentResponseStatus,
    queue_deadline: String,
    turn_sequence: Option<u64>,
    observed_cursor: Option<u64>,
    delivery: Option<AgentOperationDelivery>,
    failure: Option<CommandFailureCode>,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "CommandReceiptView")]
struct ReceiptFields {
    command_id: AgentCommandId,
    agent_id: AgentId,
    response_id: Option<AgentResponseId>,
    admitted_at: String,
    agent_order: u64,
    event_cursor: u64,
}
#[derive(Serialize, Deserialize)]
pub(super) struct ControlStatus {
    control: Status,
    target: TargetedResponse,
    expected_agent_revision: u64,
}
#[derive(Serialize, Deserialize)]
pub(super) struct InterruptStatus {
    control: Status,
    response: Status,
    target: TargetedResponse,
    expected_agent_revision: u64,
}
#[derive(Serialize, Deserialize)]
pub(super) struct EditStatus {
    edit: Status,
    response: Status,
    binding: QueuedInputBinding,
}
#[derive(Serialize, Deserialize)]
pub(super) struct CancelStatus {
    cancellation: Status,
    response: Status,
    target: QueuedCancellationTarget,
}
impl From<OwnerCommandReply> for Reply {
    fn from(value: OwnerCommandReply) -> Self {
        match value {
            OwnerCommandReply::Admit(value) => {
                Self::Admit(value.map(Status).map_err(AdmissionFailure))
            }
            OwnerCommandReply::TargetedControl(value) => Self::TargetedControl(
                value
                    .map(|v| ControlStatus {
                        control: Status(v.control),
                        target: v.target,
                        expected_agent_revision: v.expected_agent_revision,
                    })
                    .map_err(ControlFailure),
            ),
            OwnerCommandReply::DirectTargetedControl(value) => Self::DirectTargetedControl(
                value
                    .map(|v| ControlStatus {
                        control: Status(v.control),
                        target: v.target,
                        expected_agent_revision: v.expected_agent_revision,
                    })
                    .map_err(AdmissionFailure),
            ),
            OwnerCommandReply::InterruptingMessage(value) => Self::InterruptingMessage(
                value
                    .map(|v| InterruptStatus {
                        control: Status(v.control),
                        response: Status(v.response),
                        target: v.target,
                        expected_agent_revision: v.expected_agent_revision,
                    })
                    .map_err(ControlFailure),
            ),
            OwnerCommandReply::QueuedInput(value) => Self::QueuedInput(
                value
                    .map(|v| EditStatus {
                        edit: Status(v.edit),
                        response: Status(v.response),
                        binding: v.binding,
                    })
                    .map_err(QueueFailure),
            ),
            OwnerCommandReply::QueuedCancellation(value) => Self::QueuedCancellation(
                value
                    .map(|v| CancelStatus {
                        cancellation: Status(v.cancellation),
                        response: Status(v.response),
                        target: v.target,
                    })
                    .map_err(QueueFailure),
            ),
        }
    }
}
impl From<Reply> for OwnerCommandReply {
    fn from(value: Reply) -> Self {
        match value {
            Reply::Admit(value) => Self::Admit(value.map(|v| v.0).map_err(|e| e.0)),
            Reply::TargetedControl(value) => Self::TargetedControl(
                value
                    .map(|v| TargetedControlStatusView {
                        control: v.control.0,
                        target: v.target,
                        expected_agent_revision: v.expected_agent_revision,
                    })
                    .map_err(|e| e.0),
            ),
            Reply::DirectTargetedControl(value) => Self::DirectTargetedControl(
                value
                    .map(|v| TargetedControlStatusView {
                        control: v.control.0,
                        target: v.target,
                        expected_agent_revision: v.expected_agent_revision,
                    })
                    .map_err(|e| e.0),
            ),
            Reply::InterruptingMessage(value) => Self::InterruptingMessage(
                value
                    .map(|v| InterruptingMessageStatusView {
                        control: v.control.0,
                        response: v.response.0,
                        target: v.target,
                        expected_agent_revision: v.expected_agent_revision,
                    })
                    .map_err(|e| e.0),
            ),
            Reply::QueuedInput(value) => Self::QueuedInput(
                value
                    .map(|v| QueuedInputStatusView {
                        edit: v.edit.0,
                        response: v.response.0,
                        binding: v.binding,
                    })
                    .map_err(|e| e.0),
            ),
            Reply::QueuedCancellation(value) => Self::QueuedCancellation(
                value
                    .map(|v| QueuedCancellationStatusView {
                        cancellation: v.cancellation.0,
                        response: v.response.0,
                        target: v.target,
                    })
                    .map_err(|e| e.0),
            ),
        }
    }
}
