use super::{CommandStatus, QueuedCancellationTarget};
use serde::{Deserialize, Serialize};
/// Replacement text with exact original source revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedInputRequest {
    /// Original command identity.
    pub source_command_id: String,
    /// Revision observed before editing.
    pub expected_command_revision: u64,
    /// Complete replacement content, including packed attachment text.
    pub text: String,
}
/// One pending input in durable admission order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedInputItem {
    /// Original command/response identity and current safe status.
    pub response: CommandStatus,
    /// Current optimistic revision.
    pub command_revision: u64,
    /// Current complete input text.
    pub text: String,
    /// Current eligibility observation; mutation always checks again.
    pub editable: bool,
}
string_enum! { /// Authoritative queue discriminator.
    pub enum QueuedInputPageObject { Queue => "cma.agent.queued_inputs" }
}
/// Queue page; the high-level reader accumulates all bounded pages.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedInputPage {
    /// Document discriminator.
    pub object: QueuedInputPageObject,
    /// Exact requested Agent.
    pub agent_id: String,
    /// Entries ordered by original admission position.
    pub data: Vec<QueuedInputItem>,
    /// Exclusive admission-order cursor; null after the final page.
    pub next_after: Option<u64>,
}
string_enum! { /// Applied input edit discriminator.
    pub enum QueuedInputObject { Edit => "cma.response.queued_input" }
}
/// Immutable edit receipt and current source status.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedInput {
    /// Document discriminator.
    pub object: QueuedInputObject,
    /// Terminal edit command receipt.
    pub edit: CommandStatus,
    /// Source response's current safe status.
    pub response: CommandStatus,
    /// Original immutable source selection.
    pub target: QueuedCancellationTarget,
    /// Source revision immediately after this edit committed.
    pub command_revision: u64,
    /// Original replacement bytes retained for exact recovery.
    pub text: String,
}
