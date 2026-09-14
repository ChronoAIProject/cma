//! Attribution and receipts for the separate command ledger. These are server
//! records, never accepted in place of authenticated command authorization.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{AgentCommandId, AgentResponseId};
use crate::sandbox::{AgentOperationError, AgentOperationKind};
use crate::AgentId;

use super::{AgentCommand, AGENT_IO_SCHEMA_VERSION};

pub use crate::resource_access::{ResourceActor as AgentActor, ResourceOwner as AgentOwner};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSourceKind {
    UserInterface,
    Api,
    Trigger,
    Channel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCommandSource {
    pub kind: AgentSourceKind,
    pub reference: String,
}

/// The stable authorization identity survives credential rotation. Revisions
/// record the fresh decision but do not create a fresh idempotency namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAuthorizationRevision {
    pub authority_id: String,
    pub policy_revision: u64,
    pub grant_revision: Option<u64>,
}

impl AgentAuthorizationRevision {
    pub fn idempotency_scope(&self, agent_id: &AgentId, kind: AgentOperationKind) -> String {
        // Length-delimited serialization avoids delimiter and prefix collisions.
        let bytes = serde_json::to_vec(&(&self.authority_id, agent_id, kind))
            .expect("typed idempotency scope is serializable");
        format!("{:x}", Sha256::digest(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCommandEnvelope {
    pub schema_version: u16,
    pub command_id: AgentCommandId,
    pub agent_id: AgentId,
    pub owner: AgentOwner,
    pub actor: AgentActor,
    pub source: AgentCommandSource,
    pub correlation_id: String,
    pub request_fingerprint: String,
    pub authorization: AgentAuthorizationRevision,
    pub command: AgentCommand,
}

impl AgentCommandEnvelope {
    /// Integrity checking is not authorization. Only the authorization service
    /// may select these identity fields before the ledger writes this record.
    pub fn validate_integrity(&self) -> Result<(), AgentOperationError> {
        if self.schema_version != AGENT_IO_SCHEMA_VERSION
            || self.command_id.as_str().is_empty()
            || self.agent_id.as_str().is_empty()
            || self.correlation_id.trim().is_empty()
            || self.correlation_id.len() > 256
            || self.authorization.authority_id.trim().is_empty()
            || self.authorization.policy_revision == 0
            || self.authorization.grant_revision == Some(0)
            || self.request_fingerprint != self.command.fingerprint()?
        {
            return Err(AgentOperationError::Corrupt(
                "command envelope integrity is invalid".into(),
            ));
        }
        Ok(())
    }
}

/// Immutable acceptance facts. Delivery confirmation is separate from turn
/// completion, and a live stream is not evidence that this receipt was stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionReceipt {
    pub envelope: AgentCommandEnvelope,
    pub response_id: Option<AgentResponseId>,
    pub admitted_at: String,
    pub agent_order: u64,
    pub event_cursor: u64,
}

impl AdmissionReceipt {
    pub fn validate_integrity(&self) -> Result<(), AgentOperationError> {
        self.envelope.validate_integrity()?;
        if self.agent_order == 0
            || crate::seconds_from_rfc3339(&self.admitted_at).is_none()
            || (matches!(self.envelope.command, AgentCommand::Message { .. })
                && self.response_id.is_none())
            || self
                .response_id
                .as_ref()
                .is_some_and(|id| id.as_str().is_empty())
        {
            return Err(AgentOperationError::Corrupt(
                "admission receipt integrity is invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentResponseStatus {
    Queued,
    InProgress,
    WaitingAction,
    Completed,
    Failed,
    Cancelled,
}

impl AgentResponseStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}
