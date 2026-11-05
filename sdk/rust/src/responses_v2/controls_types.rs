//! Exact response-control intents and safe durable command observations.
use serde::{Deserialize, Serialize};

string_enum! { /// Approval resolution on the selected response turn.
    pub enum ApprovalDecision { Allow => "allow", Deny => "deny", Abort => "abort" }
}
/// One immutable response-targeted control intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlCommand {
    /// Request interruption of this response's bound turn.
    Cancel {},
    /// Steer this response's bound turn with unchanged text bytes.
    Steer {
        /// Text to deliver to the selected turn.
        text: String,
    },
    /// Resolve only the current pending approval on the selected turn.
    ResolveApproval {
        /// Exact approval identity from the authorized companion observation.
        approval_id: String,
        /// Explicit allow, deny or abort decision.
        decision: ApprovalDecision,
    },
}
/// Preserve the complete body and key when recovering an uncertain admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    /// Original bound response turn from the controls observation.
    pub turn_sequence: u64,
    /// Agent aggregate revision, distinct from its profile revision.
    pub expected_agent_revision: u64,
    /// Exact command intent.
    pub command: ControlCommand,
}
string_enum! { /// Durable command status; completed control means delivery.
    pub enum CommandState { Queued => "queued", InProgress => "in_progress", WaitingAction => "waiting_action", Completed => "completed", Failed => "failed", Cancelled => "cancelled" }
}
impl CommandState {
    /// Terminal command status does not establish the selected response outcome.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}
string_enum! { /// Observed runtime delivery; uncertainty remains explicit.
    pub enum CommandDelivery { DispatchFenced => "dispatch_fenced", WriteConfirmed => "write_confirmed", ExecutionObserved => "execution_observed", PreWriteFailed => "pre_write_failed", RecoveryRequired => "recovery_required", RuntimeLost => "runtime_lost" }
}
string_enum! { /// Safe durable command failure; no runtime details or credentials.
    pub enum CommandFailure { QueueTimeout => "queue_timeout", AuthorityRevoked => "authority_revoked", AuthorityUnavailable => "authority_unavailable", CredentialSetupRequired => "credential_setup_required", ConnectionsRequired => "connections_required", AgentUnavailable => "agent_unavailable", TargetTurnChanged => "target_turn_changed", RuntimeUnavailable => "runtime_unavailable", RuntimeLost => "runtime_lost", ExecutionFailed => "execution_failed" }
}
string_enum! { /// Safe command document discriminator.
    pub enum CommandObject { Command => "cma.agent.command" }
}
string_enum! { /// Response control observation discriminator.
    pub enum ResponseControlsObject { Controls => "cma.response.controls" }
}
string_enum! { /// Accepted response control discriminator.
    pub enum ResponseControlObject { Control => "cma.response.control" }
}
/// Durable admission facts without command input, actor, owner or custody.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandReceipt {
    /// Stable command identity for status polling.
    pub command_id: String,
    /// Exact Agent identity.
    pub agent_id: String,
    /// Source message response, or explicit null for a control.
    #[serde(deserialize_with = "required_nullable")]
    pub response_id: Option<String>,
    /// Original admission timestamp.
    pub admitted_at: String,
    /// Durable order in this Agent's command ledger.
    pub agent_order: u64,
    /// Admission event cursor; not a Responses SSE sequence.
    pub event_cursor: u64,
}
/// Safe status of a source message or control command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandStatus {
    /// Document discriminator.
    pub object: CommandObject,
    /// Immutable admission receipt.
    pub receipt: CommandReceipt,
    /// Current command status.
    pub status: CommandState,
    /// Original nonrenewing queue deadline.
    pub queue_deadline: String,
    /// Bound turn, or explicit null before dispatch.
    #[serde(deserialize_with = "required_nullable")]
    pub turn_sequence: Option<u64>,
    /// Last observed event cursor, or explicit null.
    #[serde(deserialize_with = "required_nullable")]
    pub observed_cursor: Option<u64>,
    /// Delivery observation, including unresolved recovery states.
    #[serde(deserialize_with = "required_nullable")]
    pub delivery: Option<CommandDelivery>,
    /// Safe failure code, or explicit null.
    #[serde(deserialize_with = "required_nullable")]
    pub failure: Option<CommandFailure>,
}
/// Exact response and original bound turn selected by a control.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseControlTarget {
    /// Original response identity.
    pub response_id: String,
    /// Original bound turn.
    pub turn_sequence: u64,
}
/// Runtime support observation; does not grant caller authorization.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRuntimeSupport {
    /// Runtime interrupt support.
    pub cancel: bool,
    /// Runtime steer support.
    pub steer: bool,
    /// Runtime approval resolution support.
    pub resolve_approval: bool,
}
/// Current safe source-command, revision and runtime support observation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseControls {
    /// Document discriminator.
    pub object: ResponseControlsObject,
    /// Safe source message command status.
    pub response: CommandStatus,
    /// Explicit null while the response has no bound turn.
    #[serde(deserialize_with = "required_nullable")]
    pub target: Option<ResponseControlTarget>,
    /// Current Agent aggregate revision.
    pub agent_revision: u64,
    /// Whether the selected turn was active at observation time.
    pub target_active: bool,
    /// Runtime support, independent of current caller permissions.
    pub runtime_support: ControlRuntimeSupport,
}
/// Accepted command; actual cancellation needs the target response's durable outcome.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseControl {
    /// Document discriminator.
    pub object: ResponseControlObject,
    /// Control delivery status. Its own receipt response ID is null.
    pub control: CommandStatus,
    /// Original immutable target retained through retries.
    pub target: ResponseControlTarget,
    /// Originally submitted Agent revision, including on receipt recovery.
    pub expected_agent_revision: u64,
}
fn required_nullable<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}
