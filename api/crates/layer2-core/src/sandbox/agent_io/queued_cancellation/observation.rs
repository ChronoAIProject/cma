use crate::resource_access::{AccessPurpose, CallerContext, ResourceCapability, ResourceTarget};
use crate::{ids::AgentResponseId, AgentId};

use super::super::admission_service::bounded;
use super::super::{
    AgentCommandService, CommandAdmissionError, CommandLedgerError, CommandStatusView,
};
use super::{queued_cancellation_eligible, QueuedCancellationObservation};

impl AgentCommandService {
    /// Eligibility is observed under current human/API operation authority.
    /// It does not grant a later mutation or external Channel sender authority.
    pub async fn observe_queued_cancellation(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        agent_id: &AgentId,
        response_id: &AgentResponseId,
    ) -> Result<QueuedCancellationObservation, CommandAdmissionError> {
        if !context.validate_principal() || context.actor.ordinary_subject().is_none() {
            return Err(CommandLedgerError::NotFound.into());
        }
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let capability = ResourceCapability::OperateAgent;
        let authorized = bounded(self.access.authorize(
            context,
            credential,
            &target,
            capability,
            AccessPurpose::Read,
        ))
        .await??;
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store.get_response_command(response_id),
            )
            .await;
        self.fence(&authorized, &target, capability).await?;
        let response = result??.ok_or(CommandLedgerError::NotFound)?;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(CommandLedgerError::Corrupt)?;
        let owner = authorized
            .snapshot
            .scope(&target)
            .map_err(crate::resource_access::ResourceAccessError::from)?
            .owner;
        super::reservation::validate_source(agent, &owner, &response, response_id)?;
        Ok(QueuedCancellationObservation {
            response: CommandStatusView::from_record(&response),
            command_revision: response.revision,
            cancellable: queued_cancellation_eligible(&response),
        })
    }
}
