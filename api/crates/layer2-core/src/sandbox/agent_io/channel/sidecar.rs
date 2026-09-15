use serde::{Deserialize, Serialize};

use crate::ids::{AgentCommandId, AgentResponseId};

use super::{ChannelCommandAdmissionError, ChannelCommandReservation};

/// Versioned Channel-specific data lives outside the ordinary command record.
/// Historical correlation and a durable hold are not future execution authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelCommandSidecar {
    pub schema_version: u16,
    pub inbox_id: String,
    pub allocation_id: String,
    pub body_sha256: String,
    pub command_id: AgentCommandId,
    pub response_id: AgentResponseId,
    pub request_fingerprint: String,
    pub execution_hold: ChannelCommandHold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelCommandHold {
    WorkerUnavailable,
    Released,
}

impl ChannelCommandReservation {
    pub fn sidecar(&self) -> Result<ChannelCommandSidecar, ChannelCommandAdmissionError> {
        Ok(ChannelCommandSidecar {
            schema_version: 1,
            inbox_id: self.inbox.id.clone(),
            allocation_id: self.allocation.id.clone(),
            body_sha256: self.inbox.attestation.body_sha256.clone(),
            command_id: self.reservation.envelope.command_id.clone(),
            response_id: self
                .reservation
                .response_id
                .clone()
                .ok_or(ChannelCommandAdmissionError::SourceMismatch)?,
            request_fingerprint: self.reservation.envelope.request_fingerprint.clone(),
            execution_hold: ChannelCommandHold::WorkerUnavailable,
        })
    }
}
