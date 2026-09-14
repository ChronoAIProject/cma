use std::collections::BTreeMap;

use crate::ids::AgentResponseId;
use crate::resource_access::{ResourceAccessError, ResourceCapability, ResourceTarget};
use crate::sandbox::AgentEventStreamAuthorization;

use super::*;

/// A response's safe durable acceptance state. Personal callers may also read
/// their submitted metadata; recipient history keeps that metadata private.
pub struct AgentResponseReading {
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
        let AgentCommand::Message { metadata, .. } = &record.receipt.envelope.command else {
            return Err(CommandLedgerError::Corrupt.into());
        };
        Ok(AgentResponseReading {
            command: CommandStatusView::from_record(&record),
            metadata: if personal {
                metadata.clone()
            } else {
                BTreeMap::new()
            },
        })
    }
}
