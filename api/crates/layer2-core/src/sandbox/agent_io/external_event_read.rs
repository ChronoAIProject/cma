use super::{
    admission_service::bounded, external_event_key, AgentCommandService, CommandAdmissionError,
    CommandLedgerError, ExternalEventContext,
};
use crate::{
    resource_access::{AccessPurpose, CallerContext, ResourceCapability, ResourceTarget},
    AgentId,
};

impl AgentCommandService {
    /// Only current binding authority can read its own retained external context.
    /// Revalidate after buffering so rotation or retirement can revoke the read.
    pub async fn external_event_context(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        agent: &AgentId,
        binding: &str,
        conversation: &str,
        event: &str,
    ) -> Result<ExternalEventContext, CommandAdmissionError> {
        let target = ResourceTarget::Agent {
            agent_id: agent.clone(),
        };
        let capability = ResourceCapability::ViewHistory;
        let authorized = bounded(self.access.authorize(
            context,
            credential,
            &target,
            capability,
            AccessPurpose::Read,
        ))
        .await??;
        let proof = authorized
            .binding_authority()
            .ok_or(CommandLedgerError::NotFound)?;
        if proof.binding.binding_id != binding || proof.conversation.conversation_id != conversation
        {
            return Err(CommandLedgerError::NotFound.into());
        }
        let key = external_event_key(&format!("binding:{binding}"), conversation, event);
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store.get_external_event_command(&key),
            )
            .await;
        self.fence(&authorized, &target, capability).await?;
        let record = result??.ok_or(CommandLedgerError::NotFound)?;
        record.validate()?;
        proof.require_command(&record.receipt.envelope)?;
        if record.receipt.envelope.external_event_key().as_ref() != Some(&key) {
            return Err(CommandLedgerError::Corrupt.into());
        }
        record
            .receipt
            .envelope
            .command
            .external_event()
            .cloned()
            .ok_or(CommandLedgerError::Corrupt.into())
    }
}
