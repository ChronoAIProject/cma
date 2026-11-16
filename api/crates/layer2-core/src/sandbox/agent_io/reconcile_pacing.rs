//! Read-only scheduling hints. A hint never authorizes a state transition;
//! reconciliation still validates the authoritative snapshot and complete log.
use super::{AdmittedAgentCommand, AgentCommand, AgentResponseStatus};
use crate::sandbox::AgentOperationDelivery;
use crate::AgentId;

pub const MAX_COMMAND_RECONCILE_HEADS: usize = 50;

pub(super) fn recheck_delay(
    status: AgentResponseStatus,
    configured: std::time::Duration,
) -> std::time::Duration {
    if status.is_terminal() {
        std::time::Duration::ZERO
    } else {
        configured
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandReconcileHead {
    pub agent_id: AgentId,
    pub last_seq: u64,
    pub live: bool,
    pub runtime_generation: Option<u64>,
    pub active_turn: Option<u64>,
}

impl AdmittedAgentCommand {
    pub fn reconciliation_current(&self, head: &CommandReconcileHead) -> bool {
        let Some(dispatch) = &self.work.dispatch else {
            return false;
        };
        let Some(observation) = &self.work.observation else {
            return false;
        };
        !self.status.is_terminal()
            && self.work.failure.is_none()
            && observation.event_cursor == head.last_seq
            && head.agent_id == dispatch.claim.agent_id
            && head.live
            && head.runtime_generation == Some(dispatch.runtime_generation)
            && (!matches!(self.receipt.envelope.command, AgentCommand::Message { .. })
                || head.active_turn == Some(dispatch.turn_sequence))
            && (!matches!(
                observation.delivery,
                AgentOperationDelivery::WriteConfirmed | AgentOperationDelivery::ExecutionObserved
            ) || self.work.lease.is_none())
    }
}
