use crate::sandbox::{store::SandboxMutationGuard, Agent, AgentLogEntry, AgentLogKind, Sandbox};

use super::super::{AdmittedAgentCommand, AgentCommand, CommandLedgerError, CommandReservation};
use super::{AdmittedTargetedControl, TargetedControlBinding, TargetedResponse};

#[derive(Debug, Clone)]
pub struct TargetedControlReservation {
    pub command: CommandReservation,
    pub target: TargetedResponse,
    /// Fresh internal authorization observation, independent of original intent.
    pub observed_agent_revision: u64,
    pub agent_created_at: String,
    /// Request-local retry scheduling budget shared by inspection and admission.
    /// This does not alter credential authority, fingerprints or persisted receipts.
    pub retry_deadline: Option<tokio::time::Instant>,
}

impl TargetedControlReservation {
    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        self.command.require_non_channel()?;
        self.command.validate()?;
        self.fingerprint()?;
        if self.command.response_id.is_some()
            || !(1..=i64::MAX as u64).contains(&self.observed_agent_revision)
            || crate::seconds_from_rfc3339(&self.agent_created_at).is_none()
        {
            return Err(CommandLedgerError::Invalid);
        }
        Ok(())
    }

    pub fn fingerprint(&self) -> Result<String, CommandLedgerError> {
        self.target.fingerprint(
            &self.command.envelope.agent_id,
            self.command.expected_agent_revision,
            &self.command.envelope.command,
        )
    }

    /// Required for both creation and historical replay, without active-turn or
    /// queue-state requirements. The store also checks the current policy.
    pub fn validate_current(
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
        if self.observed_agent_revision != agent.revision
            || self.command.parent != SandboxMutationGuard::capture(parent)
        {
            return Err(CommandLedgerError::Conflict);
        }
        Ok(())
    }

    /// No target-state checks before this permanent-key comparison. In
    /// particular, a legacy v1 receipt cannot be retroactively bound to a target.
    pub fn replay(
        &self,
        record: AdmittedAgentCommand,
        binding: Option<TargetedControlBinding>,
    ) -> Result<AdmittedTargetedControl, CommandLedgerError> {
        self.command.replay(record.clone())?;
        let binding = binding.ok_or(CommandLedgerError::IdempotencyConflict)?;
        binding.validate(&record)?;
        if binding.request_fingerprint != self.fingerprint()? {
            return Err(CommandLedgerError::IdempotencyConflict);
        }
        if binding.agent_created_at != self.agent_created_at {
            return Err(CommandLedgerError::NotFound);
        }
        Ok(AdmittedTargetedControl { record, binding })
    }

    /// Verify the original bound source without requiring a still-active turn.
    pub fn validate_source(
        &self,
        agent: &Agent,
        response: &AdmittedAgentCommand,
    ) -> Result<(), CommandLedgerError> {
        response.validate()?;
        if response.receipt.envelope.agent_id != agent.id
            || response.receipt.envelope.owner != self.command.envelope.owner
            || response.receipt.response_id.as_ref() != Some(&self.target.response_id)
        {
            return Err(CommandLedgerError::NotFound);
        }
        // A retained response from an earlier incarnation of the same opaque
        // Agent ID cannot become a target in the newly created resource.
        let parse = |value: &str| {
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                .map_err(|_| CommandLedgerError::Corrupt)
        };
        if parse(&response.receipt.admitted_at)? < parse(&agent.created_at)? {
            return Err(CommandLedgerError::NotFound);
        }
        if !matches!(
            response.receipt.envelope.command,
            AgentCommand::Message { .. }
        ) || response
            .work
            .dispatch
            .as_ref()
            .is_none_or(|dispatch| dispatch.turn_sequence != self.target.turn_sequence)
        {
            return Err(CommandLedgerError::NotAdmissible);
        }
        Ok(())
    }

    pub fn validate_target(
        &self,
        agent: &Agent,
        response: &AdmittedAgentCommand,
    ) -> Result<(), CommandLedgerError> {
        self.validate_source(agent, response)?;
        if response.status.is_terminal()
            || agent.conversation.active_turn != Some(self.target.turn_sequence)
        {
            return Err(CommandLedgerError::NotAdmissible);
        }
        if self.command.expected_agent_revision != agent.revision {
            return Err(CommandLedgerError::Conflict);
        }
        Ok(())
    }

    pub fn validate_approval(&self, pending: &[AgentLogEntry]) -> Result<(), CommandLedgerError> {
        if let AgentCommand::AnswerApproval { approval_id, .. } = &self.command.envelope.command {
            if !pending.iter().any(|entry| {
                entry.agent_id == self.command.envelope.agent_id
                    && entry.seq > self.target.turn_sequence
                    && matches!(&entry.kind, AgentLogKind::ApprovalRequested { approval_id: id, .. } if id == approval_id)
            }) {
                return Err(CommandLedgerError::NotAdmissible);
            }
        }
        Ok(())
    }

    pub fn bind(
        &self,
        record: AdmittedAgentCommand,
        response: &AdmittedAgentCommand,
    ) -> Result<AdmittedTargetedControl, CommandLedgerError> {
        let binding = TargetedControlBinding {
            schema_version: 1,
            command_id: record.receipt.envelope.command_id.clone(),
            request_fingerprint: self.fingerprint()?,
            response_id: self.target.response_id.clone(),
            response_command_id: response.receipt.envelope.command_id.clone(),
            turn_sequence: self.target.turn_sequence,
            expected_agent_revision: self.command.expected_agent_revision,
            agent_created_at: self.agent_created_at.clone(),
        };
        binding.validate_response(&record, response)?;
        Ok(AdmittedTargetedControl { record, binding })
    }
}
