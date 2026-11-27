use std::collections::BTreeMap;

use crate::ids::AgentResponseId;
use crate::resource_access::{ResourceAccessError, ResourceCapability, ResourceTarget};
use crate::sandbox::{AgentEventStreamAuthorization, AgentEventStreamHeader, AgentReadError};

use super::*;

/// A response's safe durable acceptance state. Personal callers may also read
/// their submitted metadata; recipient history keeps that metadata private.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AgentResponseReading {
    pub deferred_failure_detail: Option<String>,
    pub command: CommandStatusView,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ObservedResponseReadError {
    #[error(transparent)]
    Command(#[from] CommandAdmissionError),
    #[error(transparent)]
    Header(#[from] AgentReadError),
}

impl AgentCommandService {
    /// Observe under an existing nonrenewing history authorization. No request
    /// credential is retained by a stream and no poll renews its membership.
    pub async fn response_for_stream(
        &self,
        authorization: &AgentEventStreamAuthorization,
        id: &AgentResponseId,
    ) -> Result<AgentResponseReading, CommandAdmissionError> {
        let (authorized, agent_id, _) = authorization.resource();
        let capability = ResourceCapability::ViewHistory;
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let result = self
            .with_authority_bound(authorized, capability, self.store.get_response_command(id))
            .await;
        self.fence(authorized, &target, capability).await?;
        let record = result??.ok_or(CommandLedgerError::NotFound)?;
        Self::reading(authorization, id, &record)
    }
    /// Fresh cohorts can share source records while each subscriber retains its
    /// own current release fence and redaction. The same fresh snapshot that
    /// fences the command lookup supplies the current history header.
    pub async fn response_for_observed_stream(
        &self,
        reads: &crate::sandbox::AgentReadService,
        authorization: &AgentEventStreamAuthorization,
        id: &AgentResponseId,
    ) -> Result<(AgentResponseReading, AgentEventStreamHeader), ObservedResponseReadError> {
        let record = reads.observed_command_unreleased(authorization, id).await;
        // Fence both the command result and its errors before exposing them.
        let header = reads.current_event_stream_header(authorization).await?;
        let record =
            record.map_err(|_| CommandAdmissionError::Ledger(CommandLedgerError::Unavailable))?;
        Ok((Self::reading(authorization, id, &record)?, header))
    }
    fn reading(
        authorization: &AgentEventStreamAuthorization,
        id: &AgentResponseId,
        record: &AdmittedAgentCommand,
    ) -> Result<AgentResponseReading, CommandAdmissionError> {
        let (authorized, agent_id, personal) = authorization.resource();
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        record.validate()?;
        if record.receipt.response_id.as_ref() != Some(id) {
            return Err(CommandLedgerError::Corrupt.into());
        }
        if &record.receipt.envelope.agent_id != agent_id
            || record.receipt.envelope.owner
                != authorized
                    .snapshot
                    .scope(&target)
                    .map_err(ResourceAccessError::from)?
                    .owner
        {
            return Err(CommandLedgerError::NotFound.into());
        }
        let AgentCommand::Message { .. } = &record.receipt.envelope.command else {
            return Err(CommandLedgerError::Corrupt.into());
        };
        Ok(AgentResponseReading {
            deferred_failure_detail: record
                .receipt
                .envelope
                .continuation
                .as_ref()
                .and(record.work.dispatch.as_ref())
                .map(|dispatch| {
                    super::continuation_retry::deferred_detail(
                        &record.receipt.envelope,
                        dispatch.operation_sequence,
                        dispatch.turn_sequence,
                    )
                }),
            command: CommandStatusView::from_record(record),
            metadata: if personal || authorized.binding_authority().is_some() {
                record.receipt.envelope.response_metadata()
            } else {
                BTreeMap::new()
            },
        })
    }
}
