use crate::sandbox::{Agent, Sandbox};
use crate::AgentId;

use super::{
    AdmittedAgentCommand, AdmittedTargetedControl, AgentCommand, AgentInputOrder,
    CommandLedgerError, CommandReservation, CommandStatusView, TargetedControlBinding,
    TargetedControlReservation, TargetedResponse,
};

mod service;

#[derive(Debug, Clone)]
pub struct InterruptingMessageSubmission {
    pub agent_id: AgentId,
    pub target: TargetedResponse,
    pub expected_agent_revision: u64,
    pub text: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone)]
pub struct InterruptingMessageReservation {
    pub control: TargetedControlReservation,
    pub message: CommandReservation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedInterruptingMessage {
    pub control: AdmittedTargetedControl,
    pub message: AdmittedAgentCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptingMessageStatusView {
    pub control: CommandStatusView,
    pub response: CommandStatusView,
    pub target: TargetedResponse,
    pub expected_agent_revision: u64,
}

impl From<AdmittedInterruptingMessage> for InterruptingMessageStatusView {
    fn from(value: AdmittedInterruptingMessage) -> Self {
        Self {
            control: CommandStatusView::from_record(&value.control.record),
            response: CommandStatusView::from_record(&value.message),
            target: value.control.binding.target(),
            expected_agent_revision: value.control.binding.expected_agent_revision,
        }
    }
}

impl InterruptingMessageReservation {
    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        self.control.validate()?;
        self.message.validate()?;
        self.message.require_non_channel()?;
        let control = &self.control.command;
        if control.envelope.command != AgentCommand::Interrupt
            || self.message.response_id.is_none()
            || self.message.envelope.agent_id != control.envelope.agent_id
            || self.message.envelope.owner != control.envelope.owner
            || self.message.envelope.actor != control.envelope.actor
            || self.message.envelope.source.kind != control.envelope.source.kind
            || self.message.binding != control.binding
            || self.message.envelope.authorization != control.envelope.authorization
            || self.message.key_sha256 != control.key_sha256
            || self.message.expected_agent_revision != control.expected_agent_revision
            || self.message.parent != control.parent
            || self.message.policy != control.policy
            || self.message.queue_capacity != control.queue_capacity
            || !matches!(self.message.envelope.command, AgentCommand::Message {
                order: AgentInputOrder::InterruptAfterTurn { turn_sequence }, ..
            } if turn_sequence == self.control.target.turn_sequence)
        {
            return Err(CommandLedgerError::Invalid);
        }
        Ok(())
    }

    pub fn require_current_authority(&self) -> Result<(), CommandLedgerError> {
        self.control.command.require_current_authority()?;
        self.message.require_current_authority()
    }

    pub fn validate_current(
        &self,
        agent: &Agent,
        parent: &Sandbox,
    ) -> Result<(), CommandLedgerError> {
        self.require_current_authority()?;
        self.control.validate_current(agent, parent)
    }

    /// Compare both permanent keys before checking whether the source is still
    /// active. Retrying a delivered interruption never creates another message.
    pub fn replay(
        &self,
        control: AdmittedTargetedControl,
        message: AdmittedAgentCommand,
    ) -> Result<AdmittedInterruptingMessage, CommandLedgerError> {
        let control = self.control.replay(control.record, Some(control.binding))?;
        self.message.replay(message.clone())?;
        if message.receipt.agent_order
            != control
                .record
                .receipt
                .agent_order
                .checked_add(1)
                .ok_or(CommandLedgerError::Corrupt)?
            || message.key_sha256 != control.record.key_sha256
            || message.receipt.envelope.agent_id != control.record.receipt.envelope.agent_id
        {
            return Err(CommandLedgerError::IdempotencyConflict);
        }
        Ok(AdmittedInterruptingMessage { control, message })
    }

    pub fn replay_records(
        &self,
        control: AdmittedAgentCommand,
        binding: Option<TargetedControlBinding>,
        message: AdmittedAgentCommand,
    ) -> Result<AdmittedInterruptingMessage, CommandLedgerError> {
        self.replay(self.control.replay(control, binding)?, message)
    }
}
