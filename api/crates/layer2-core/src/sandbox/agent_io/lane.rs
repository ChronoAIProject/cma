use serde::{Deserialize, Serialize};

use crate::sandbox::{Agent, AgentStatus};
use crate::seconds_from_rfc3339;

use super::{
    AdmittedAgentCommand, AgentCommand, AgentResponseStatus, CommandClaim, CommandClaimRequest,
    CommandLease, CommandLedgerError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandLane {
    pub last_order: u64,
    #[serde(default = "first_fence")]
    pub next_fence: u64,
    #[serde(default)]
    pub lease: Option<CommandLease>,
}

const fn first_fence() -> u64 {
    1
}

impl Default for CommandLane {
    fn default() -> Self {
        Self {
            last_order: 0,
            next_fence: 1,
            lease: None,
        }
    }
}

impl CommandLane {
    pub fn claim(
        &mut self,
        command: &mut AdmittedAgentCommand,
        request: &CommandClaimRequest,
    ) -> Result<bool, CommandLedgerError> {
        request.validate()?;
        if command.receipt.agent_order > self.last_order {
            return Err(CommandLedgerError::Corrupt);
        }
        if self
            .lease
            .as_ref()
            .is_some_and(|lease| lease.live_at(&request.now))
        {
            return Ok(false);
        }
        let next = self
            .next_fence
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or(CommandLedgerError::Corrupt)?;
        command.claim(request, self.next_fence)?;
        self.lease = command.work.lease.clone();
        self.next_fence = next;
        Ok(true)
    }

    pub fn require_claim(&self, claim: &CommandClaim, now: &str) -> Result<(), CommandLedgerError> {
        if !self
            .lease
            .as_ref()
            .is_some_and(|lease| &lease.claim == claim && lease.live_at(now))
        {
            return Err(CommandLedgerError::StaleClaim);
        }
        Ok(())
    }
}

impl AdmittedAgentCommand {
    pub fn predates_owner_restart(&self, agent: &Agent) -> bool {
        agent
            .work
            .pre_restart_command_order
            .is_some_and(|order| self.receipt.agent_order <= order)
    }

    /// Stable priority applies only to the message admitted with a targeted
    /// interruption. Existing ordinary queue positions remain unchanged.
    pub fn queue_priority(&self) -> (u8, u64) {
        let priority = if matches!(
            self.receipt.envelope.command,
            AgentCommand::Message {
                order: super::AgentInputOrder::InterruptAfterTurn { .. },
                ..
            }
        ) {
            0
        } else {
            1
        };
        (priority, self.receipt.agent_order)
    }

    /// Deferred supplements and held Channel messages retain their original
    /// message position. Controls may still address an active turn.
    pub fn holds_message_order(&self, now: &str) -> bool {
        if self.awaiting_claim(now)
            && !self.queue_expired(now)
            && !self.continuation_retry_delay(now).is_zero()
        {
            return true;
        }
        self.status == AgentResponseStatus::WaitingAction
            && self.work.dispatch.is_none()
            && matches!(
                self.receipt.envelope.actor,
                super::AgentActor::Channel { .. }
            )
            && matches!(self.receipt.envelope.command, AgentCommand::Message { .. })
    }
}

/// Normal messages keep FIFO order while controls can address the active turn.
/// Expired or terminal-resource work is claimable only so it can be refused.
pub fn command_ready_for_claim(
    command: &AdmittedAgentCommand,
    agent: Option<&Agent>,
    now: &str,
) -> bool {
    if command.status != AgentResponseStatus::Queued
        || command.work.dispatch.is_some()
        || command
            .work
            .lease
            .as_ref()
            .is_some_and(|lease| lease.live_at(now))
    {
        return false;
    }
    let expired = seconds_from_rfc3339(now)
        .zip(seconds_from_rfc3339(&command.queue_deadline))
        .is_some_and(|(now, deadline)| now >= deadline);
    let Some(agent) = agent else { return true };
    if command.predates_owner_restart(agent) {
        return false;
    }
    if expired || !agent.status.is_live() || agent.requested_stop_reason.is_some() {
        return true;
    }
    if !command.continuation_retry_delay(now).is_zero() {
        return false;
    }
    if agent
        .conversation
        .operation
        .as_ref()
        .is_some_and(|operation| operation.blocks_dispatch())
    {
        return false;
    }
    match command.receipt.envelope.command {
        AgentCommand::Message { .. } => {
            !agent
                .conversation
                .turn_timeout
                .as_ref()
                .is_some_and(|timeout| timeout.blocks_dispatch())
                && agent.status == AgentStatus::Ready
                && agent.conversation.active_turn.is_none()
                && agent
                    .conversation
                    .queued_messages
                    .iter()
                    .all(|message| message.state == crate::sandbox::QueuedMessageState::Delivered)
                && agent
                    .conversation
                    .first_message
                    .as_ref()
                    .is_none_or(|first| {
                        first.is_terminal() || super::is_bound_trigger_first(command, agent)
                    })
        }
        _ => true,
    }
}
