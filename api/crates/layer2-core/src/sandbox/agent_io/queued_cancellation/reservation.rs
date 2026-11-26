use crate::sandbox::{store::SandboxMutationGuard, Agent, Sandbox};

use super::super::{
    AdmittedAgentCommand, AgentActor, AgentCommand, AgentResponseStatus, CommandLane,
    CommandLedgerError, CommandReservation, CommandWorkState,
};
use super::{AdmittedQueuedCancellation, QueuedCancellationBinding, QueuedCancellationTarget};

#[derive(Debug, Clone)]
pub struct QueuedCancellationReservation {
    pub command: CommandReservation,
    pub target: QueuedCancellationTarget,
    pub observed_agent_revision: u64,
    pub agent_created_at: String,
}

/// A live lease alone is not runtime dispatch. The transaction invalidates the
/// exact matching claim before the worker can bind any runtime input.
pub fn queued_cancellation_eligible(response: &AdmittedAgentCommand) -> bool {
    response.work.dispatch.is_none()
        && response.work.observation.is_none()
        && response.work.failure.is_none()
        && response.target_turn.is_none()
        && matches!(
            response.receipt.envelope.command,
            AgentCommand::Message { .. }
        )
        && (response.status == AgentResponseStatus::Queued
            || (response.status == AgentResponseStatus::WaitingAction
                && matches!(response.receipt.envelope.actor, AgentActor::Channel { .. })
                && response.work == CommandWorkState::default()))
}

impl QueuedCancellationReservation {
    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        self.command.require_non_channel()?;
        self.command.validate()?;
        self.target.fingerprint(&self.command.envelope.agent_id)?;
        if self.command.envelope.command != AgentCommand::Interrupt
            || self.command.response_id.is_some()
            || !(1..=i64::MAX as u64).contains(&self.observed_agent_revision)
            || crate::seconds_from_rfc3339(&self.agent_created_at).is_none()
        {
            return Err(CommandLedgerError::Invalid);
        }
        Ok(())
    }

    pub fn validate_current(
        &self,
        agent: &Agent,
        parent: &Sandbox,
    ) -> Result<(), CommandLedgerError> {
        self.validate_current_identity(agent, parent)?;
        if !self.current_observation_matches(agent, parent) {
            return Err(CommandLedgerError::Conflict);
        }
        Ok(())
    }

    pub fn current_observation_matches(&self, agent: &Agent, parent: &Sandbox) -> bool {
        self.observed_agent_revision == agent.revision
            && self.command.parent == SandboxMutationGuard::capture(parent)
    }

    pub fn validate_current_identity(
        &self,
        agent: &Agent,
        parent: &Sandbox,
    ) -> Result<(), CommandLedgerError> {
        self.command.require_current_authority()?;
        let owner = match &self.command.envelope.owner {
            super::super::AgentOwner::Person { subject }
            | super::super::AgentOwner::Organization { subject } => subject,
        };
        if self.command.envelope.agent_id != agent.id
            || owner != &agent.owner
            || parent.id != agent.sandbox_id
            || parent.owner != agent.owner
            || self.agent_created_at != agent.created_at
        {
            return Err(CommandLedgerError::NotFound);
        }
        Ok(())
    }

    pub fn validate_source(
        &self,
        agent: &Agent,
        response: &AdmittedAgentCommand,
    ) -> Result<(), CommandLedgerError> {
        validate_source(
            agent,
            &self.command.envelope.owner,
            response,
            &self.target.response_id,
        )?;
        if response.receipt.envelope.command_id != self.target.source_command_id {
            return Err(CommandLedgerError::NotFound);
        }
        Ok(())
    }

    pub fn cancel(
        &self,
        response: &mut AdmittedAgentCommand,
        lane: &mut CommandLane,
    ) -> Result<(), CommandLedgerError> {
        if response.revision != self.target.expected_command_revision
            || !queued_cancellation_eligible(response)
        {
            return Err(CommandLedgerError::Conflict);
        }
        if let Some(lease) = &response.work.lease {
            if lane
                .lease
                .as_ref()
                .is_some_and(|current| current.claim == lease.claim)
            {
                if lane.lease.as_ref() != Some(lease) {
                    return Err(CommandLedgerError::Corrupt);
                }
                lane.lease = None;
            } else if lease.live_at(&crate::now_rfc3339()) {
                return Err(CommandLedgerError::Corrupt);
            }
        }
        response.revision = response
            .revision
            .checked_add(1)
            .filter(|revision| *revision <= i64::MAX as u64)
            .ok_or(CommandLedgerError::Corrupt)?;
        response.status = AgentResponseStatus::Cancelled;
        response.work.lease = None;
        response.validate()
    }

    pub fn bind(
        &self,
        record: AdmittedAgentCommand,
        response: AdmittedAgentCommand,
    ) -> Result<AdmittedQueuedCancellation, CommandLedgerError> {
        let binding = QueuedCancellationBinding {
            schema_version: 1,
            command_id: record.receipt.envelope.command_id.clone(),
            target: self.target.clone(),
            request_fingerprint: self.target.fingerprint(&self.command.envelope.agent_id)?,
            agent_created_at: self.agent_created_at.clone(),
        };
        binding.validate_response(&record, &response)?;
        Ok(AdmittedQueuedCancellation {
            record,
            binding,
            response,
        })
    }

    pub fn replay(
        &self,
        record: AdmittedAgentCommand,
        binding: Option<QueuedCancellationBinding>,
        response: AdmittedAgentCommand,
    ) -> Result<AdmittedQueuedCancellation, CommandLedgerError> {
        self.command.replay(record.clone())?;
        let binding = binding.ok_or(CommandLedgerError::IdempotencyConflict)?;
        binding.validate_response(&record, &response)?;
        if binding.request_fingerprint
            != self.target.fingerprint(&self.command.envelope.agent_id)?
        {
            return Err(CommandLedgerError::IdempotencyConflict);
        }
        if binding.agent_created_at != self.agent_created_at {
            return Err(CommandLedgerError::NotFound);
        }
        Ok(AdmittedQueuedCancellation {
            record,
            binding,
            response,
        })
    }
}

pub(in crate::sandbox::agent_io) fn validate_source(
    agent: &Agent,
    owner: &super::super::AgentOwner,
    response: &AdmittedAgentCommand,
    response_id: &crate::ids::AgentResponseId,
) -> Result<(), CommandLedgerError> {
    response.validate()?;
    let parse = |value: &str| {
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
            .map_err(|_| CommandLedgerError::Corrupt)
    };
    if response.receipt.envelope.agent_id != agent.id
        || &response.receipt.envelope.owner != owner
        || response.receipt.response_id.as_ref() != Some(response_id)
        || !matches!(
            response.receipt.envelope.command,
            AgentCommand::Message { .. }
        )
        || parse(&response.receipt.admitted_at)? < parse(&agent.created_at)?
    {
        return Err(CommandLedgerError::NotFound);
    }
    Ok(())
}
