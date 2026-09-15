use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{AgentCommandId, AgentResponseId};
use crate::AgentId;

use super::super::{
    AdmittedAgentCommand, AgentCommand, AgentResponseStatus, CommandLedgerError, CommandStatusView,
    CommandWorkState,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedCancellationTarget {
    pub response_id: AgentResponseId,
    pub source_command_id: AgentCommandId,
    pub expected_command_revision: u64,
}

impl QueuedCancellationTarget {
    pub fn fingerprint(&self, agent_id: &AgentId) -> Result<String, CommandLedgerError> {
        for id in [
            self.response_id.as_str(),
            self.source_command_id.as_str(),
            agent_id.as_str(),
        ] {
            if id.trim().is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
                return Err(CommandLedgerError::Invalid);
            }
        }
        if !(1..=i64::MAX as u64).contains(&self.expected_command_revision) {
            return Err(CommandLedgerError::Invalid);
        }
        let mut bytes = b"cma.queued-response-cancellation.v1\n".to_vec();
        super::super::command::write_canonical(
            &serde_json::json!({"agent_id": agent_id, "target": self}),
            &mut bytes,
        )
        .map_err(|_| CommandLedgerError::Invalid)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Debug, Clone)]
pub struct QueuedCancellationSubmission {
    pub agent_id: AgentId,
    pub target: QueuedCancellationTarget,
    pub idempotency_key: String,
}

/// Immutable effect identity, outside the strict v1 command record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedCancellationBinding {
    pub schema_version: u16,
    pub command_id: AgentCommandId,
    pub target: QueuedCancellationTarget,
    pub request_fingerprint: String,
    pub agent_created_at: String,
}

impl QueuedCancellationBinding {
    pub fn validate(&self, record: &AdmittedAgentCommand) -> Result<(), CommandLedgerError> {
        record.validate()?;
        if self.schema_version != 1
            || self.command_id != record.receipt.envelope.command_id
            || self.command_id == self.target.source_command_id
            || self.request_fingerprint
                != self
                    .target
                    .fingerprint(&record.receipt.envelope.agent_id)
                    .map_err(|_| CommandLedgerError::Corrupt)?
            || crate::seconds_from_rfc3339(&self.agent_created_at).is_none()
            || record.receipt.envelope.command != AgentCommand::Interrupt
            || record.receipt.response_id.is_some()
            || record.target_turn.is_some()
            || record.status != AgentResponseStatus::Completed
            || record.work != CommandWorkState::default()
        {
            return Err(CommandLedgerError::Corrupt);
        }
        Ok(())
    }

    pub fn validate_response(
        &self,
        record: &AdmittedAgentCommand,
        response: &AdmittedAgentCommand,
    ) -> Result<(), CommandLedgerError> {
        self.validate(record)?;
        response.validate()?;
        if response.receipt.envelope.command_id != self.target.source_command_id
            || response.receipt.response_id.as_ref() != Some(&self.target.response_id)
            || response.receipt.envelope.agent_id != record.receipt.envelope.agent_id
            || response.receipt.envelope.owner != record.receipt.envelope.owner
            || !matches!(
                response.receipt.envelope.command,
                AgentCommand::Message { .. }
            )
        {
            return Err(CommandLedgerError::Corrupt);
        }
        // The receipt is immutable; the independently read source status is a
        // current view, never part of its idempotency fingerprint.
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedQueuedCancellation {
    pub record: AdmittedAgentCommand,
    pub binding: QueuedCancellationBinding,
    pub response: AdmittedAgentCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuedCancellationOutcome {
    Applied(Box<AdmittedQueuedCancellation>),
    /// Committed scoped absence and strictly newer source revision.
    NotAdmitted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedCancellationObservation {
    pub response: CommandStatusView,
    pub command_revision: u64,
    pub cancellable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedCancellationStatusView {
    pub cancellation: CommandStatusView,
    pub response: CommandStatusView,
    pub target: QueuedCancellationTarget,
}

impl From<AdmittedQueuedCancellation> for QueuedCancellationStatusView {
    fn from(value: AdmittedQueuedCancellation) -> Self {
        Self {
            cancellation: CommandStatusView::from_record(&value.record),
            response: CommandStatusView::from_record(&value.response),
            target: value.binding.target,
        }
    }
}
