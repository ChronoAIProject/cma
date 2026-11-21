use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{AgentCommandId, AgentResponseId};
use crate::AgentId;

use super::super::{AdmittedAgentCommand, AgentCommand, CommandLedgerError, CommandStatusView};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetedResponse {
    pub response_id: AgentResponseId,
    pub turn_sequence: u64,
}

impl TargetedResponse {
    pub(super) fn validate(&self) -> Result<(), CommandLedgerError> {
        let id = self.response_id.as_str();
        if id.trim().is_empty()
            || id.len() > 256
            || id.chars().any(char::is_control)
            || !(1..=i64::MAX as u64).contains(&self.turn_sequence)
        {
            return Err(CommandLedgerError::Invalid);
        }
        Ok(())
    }

    pub fn fingerprint(
        &self,
        agent_id: &AgentId,
        expected_agent_revision: u64,
        command: &AgentCommand,
    ) -> Result<String, CommandLedgerError> {
        self.validate()?;
        if agent_id.as_str().is_empty()
            || !(1..=i64::MAX as u64).contains(&expected_agent_revision)
            || matches!(command, AgentCommand::Message { .. })
        {
            return Err(CommandLedgerError::Invalid);
        }
        let command = command
            .canonical()
            .map_err(|_| CommandLedgerError::Invalid)?;
        let value = serde_json::json!({
            "agent_id": agent_id,
            "response_id": self.response_id,
            "turn_sequence": self.turn_sequence,
            "expected_agent_revision": expected_agent_revision,
            "command": command,
        });
        let mut bytes = b"cma.agent-control.v2\n".to_vec();
        super::super::command::write_canonical(&value, &mut bytes)
            .map_err(|_| CommandLedgerError::Invalid)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

/// Internal service intent. Actor, source and custody are trusted parameters.
#[derive(Debug, Clone)]
pub struct TargetedControlSubmission {
    pub agent_id: AgentId,
    pub target: TargetedResponse,
    pub expected_agent_revision: u64,
    pub command: AgentCommand,
    pub idempotency_key: String,
}

/// Immutable top-level sidecar, outside the strict persisted v1 record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetedControlBinding {
    pub schema_version: u16,
    pub command_id: AgentCommandId,
    pub request_fingerprint: String,
    pub response_id: AgentResponseId,
    pub response_command_id: AgentCommandId,
    pub turn_sequence: u64,
    pub expected_agent_revision: u64,
    pub agent_created_at: String,
}

impl TargetedControlBinding {
    pub fn target(&self) -> TargetedResponse {
        TargetedResponse {
            response_id: self.response_id.clone(),
            turn_sequence: self.turn_sequence,
        }
    }

    pub fn validate(&self, record: &AdmittedAgentCommand) -> Result<(), CommandLedgerError> {
        record.validate()?;
        let envelope = &record.receipt.envelope;
        let fingerprint = self
            .target()
            .fingerprint(
                &envelope.agent_id,
                self.expected_agent_revision,
                &envelope.command,
            )
            .map_err(|_| CommandLedgerError::Corrupt)?;
        if self.schema_version != 1
            || self.command_id != envelope.command_id
            || self.response_command_id.as_str().is_empty()
            || self.response_command_id == self.command_id
            || self.request_fingerprint != fingerprint
            || record.receipt.response_id.is_some()
            || record.target_turn != Some(self.turn_sequence)
            || record
                .work
                .dispatch
                .as_ref()
                .is_some_and(|dispatch| dispatch.turn_sequence != self.turn_sequence)
            || crate::seconds_from_rfc3339(&self.agent_created_at).is_none()
        {
            return Err(CommandLedgerError::Corrupt);
        }
        Ok(())
    }

    /// Historical provenance remains verifiable after the target has finished.
    pub fn validate_response(
        &self,
        record: &AdmittedAgentCommand,
        response: &AdmittedAgentCommand,
    ) -> Result<(), CommandLedgerError> {
        self.validate(record)?;
        response.validate()?;
        if response.receipt.envelope.command_id != self.response_command_id
            || response.receipt.response_id.as_ref() != Some(&self.response_id)
            || response.receipt.envelope.agent_id != record.receipt.envelope.agent_id
            || response.receipt.envelope.owner != record.receipt.envelope.owner
            || !matches!(
                response.receipt.envelope.command,
                AgentCommand::Message { .. }
            )
            || response
                .work
                .dispatch
                .as_ref()
                .is_none_or(|dispatch| dispatch.turn_sequence != self.turn_sequence)
        {
            return Err(CommandLedgerError::Corrupt);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedTargetedControl {
    pub record: AdmittedAgentCommand,
    pub binding: TargetedControlBinding,
}

/// Scoped inspection under fresh authority. A refusal is returned only after
/// the store commits a fence proving this exact stale intent cannot be admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetedControlInspection {
    Existing(Box<AdmittedTargetedControl>),
    Missing,
    NotAdmitted,
}

/// Safe acceptance and delivery view. A Completed control means delivered;
/// the target response's own durable outcome establishes cancellation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetedControlStatusView {
    pub control: CommandStatusView,
    pub target: TargetedResponse,
    pub expected_agent_revision: u64,
}

impl TargetedControlStatusView {
    pub(super) fn from_record(value: &AdmittedTargetedControl) -> Self {
        Self {
            control: CommandStatusView::from_record(&value.record),
            target: value.binding.target(),
            expected_agent_revision: value.binding.expected_agent_revision,
        }
    }
}
