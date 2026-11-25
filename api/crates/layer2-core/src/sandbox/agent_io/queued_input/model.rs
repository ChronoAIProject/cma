use super::super::{
    AdmittedAgentCommand, AgentActor, AgentCommand, AgentInput, CommandLedgerError,
    CommandStatusView, QueuedCancellationTarget,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedInputRevision {
    pub schema_version: u16,
    pub command_id: crate::ids::AgentCommandId,
    pub command_revision: u64,
    pub text: String,
}
impl QueuedInputRevision {
    pub fn validate(&self, source: &AdmittedAgentCommand) -> Result<(), CommandLedgerError> {
        if self.schema_version != 1
            || self.command_id != source.receipt.envelope.command_id
            || self.command_revision < 2
            || self.command_revision > source.revision
            || !queued_input_editable_actor(source)
        {
            return Err(CommandLedgerError::Corrupt);
        }
        AgentInput::text(&self.text)
            .canonical()
            .map_err(|_| CommandLedgerError::Corrupt)?;
        Ok(())
    }
}
pub fn queued_input_editable_actor(record: &AdmittedAgentCommand) -> bool {
    if record.receipt.envelope.continuation.is_some() {
        return false;
    }
    matches!(
        record.receipt.envelope.actor,
        AgentActor::Human { .. } | AgentActor::Api { .. }
    ) && matches!(
        record.receipt.envelope.command,
        AgentCommand::Message { .. }
    ) && record.receipt.envelope.external_event_key().is_none()
}
impl AdmittedAgentCommand {
    pub fn effective_command(&self) -> Result<AgentCommand, CommandLedgerError> {
        let mut command = self.receipt.envelope.command.clone();
        if let Some(revision) = &self.queued_input {
            revision.validate(self)?;
            let AgentCommand::Message { input, .. } = &mut command else {
                return Err(CommandLedgerError::Corrupt);
            };
            input.content = AgentInput::text(&revision.text).content;
        }
        Ok(command)
    }
}
#[derive(Debug, Clone)]
pub struct QueuedInputSubmission {
    pub agent_id: crate::AgentId,
    pub target: QueuedCancellationTarget,
    pub text: String,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedInputBinding {
    pub schema_version: u16,
    pub command_id: crate::ids::AgentCommandId,
    pub target: QueuedCancellationTarget,
    pub text: String,
    pub command_revision: u64,
    pub agent_created_at: String,
    pub request_fingerprint: String,
}
pub fn queued_input_fingerprint(
    agent: &crate::AgentId,
    target: &QueuedCancellationTarget,
    text: &str,
) -> Result<String, CommandLedgerError> {
    target.fingerprint(agent)?;
    AgentInput::text(text)
        .canonical()
        .map_err(|_| CommandLedgerError::Invalid)?;
    let bytes = serde_json::to_vec(&("cma.queued-input.v1", agent, target, text))
        .map_err(|_| CommandLedgerError::Invalid)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedQueuedInput {
    pub record: AdmittedAgentCommand,
    pub binding: QueuedInputBinding,
    pub response: AdmittedAgentCommand,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuedInputOutcome {
    Applied(Box<AdmittedQueuedInput>),
    NotAdmitted,
    /// A local observation was stale before any edit or receipt could be written.
    StaleObservation,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedInputView {
    pub response: CommandStatusView,
    pub command_revision: u64,
    pub text: String,
    pub editable: bool,
}
impl QueuedInputView {
    pub fn from_record(record: &AdmittedAgentCommand) -> Result<Self, CommandLedgerError> {
        record.validate()?;
        let AgentCommand::Message { input, .. } = record.effective_command()? else {
            return Err(CommandLedgerError::Corrupt);
        };
        Ok(Self {
            response: CommandStatusView::from_record(record),
            command_revision: record.revision,
            text: input.into_text().map_err(|_| CommandLedgerError::Corrupt)?,
            editable: queued_input_editable_actor(record)
                && super::super::queued_cancellation_eligible(record),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedInputStatusView {
    pub edit: CommandStatusView,
    pub response: CommandStatusView,
    pub binding: QueuedInputBinding,
}
impl From<AdmittedQueuedInput> for QueuedInputStatusView {
    fn from(value: AdmittedQueuedInput) -> Self {
        Self {
            edit: CommandStatusView::from_record(&value.record),
            response: CommandStatusView::from_record(&value.response),
            binding: value.binding,
        }
    }
}
