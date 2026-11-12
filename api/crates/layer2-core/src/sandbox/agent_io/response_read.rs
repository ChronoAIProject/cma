use std::collections::BTreeMap;

use crate::ids::AgentResponseId;
use crate::resource_access::{ResourceAccessError, ResourceCapability, ResourceTarget};
use crate::sandbox::AgentEventStreamAuthorization;

use super::*;

/// A response's safe durable acceptance state. Personal callers may also read
/// their submitted metadata; recipient history keeps that metadata private.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResponseReading {
    pub deferred_failure_detail: Option<String>,
    pub command: CommandStatusView,
    pub metadata: BTreeMap<String, String>,
}

impl AgentCommandService {
    /// Observe under an existing nonrenewing history authorization. No request
    /// credential is retained by a stream and no poll renews its membership.
    pub async fn response_for_stream(
        &self,
        authorization: &AgentEventStreamAuthorization,
        id: &AgentResponseId,
    ) -> Result<AgentResponseReading, CommandAdmissionError> {
        let (authorized, agent_id, personal) = authorization.resource();
        let capability = ResourceCapability::ViewHistory;
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let result = self
            .with_authority_bound(authorized, capability, self.store.get_response_command(id))
            .await;
        self.fence(authorized, &target, capability).await?;
        let record = result??.ok_or(CommandLedgerError::NotFound)?;
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
            command: CommandStatusView::from_record(&record),
            metadata: if personal || authorized.binding_authority().is_some() {
                record.receipt.envelope.response_metadata()
            } else {
                BTreeMap::new()
            },
        })
    }
}
