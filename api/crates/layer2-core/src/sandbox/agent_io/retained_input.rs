//! Authorized immutable input lookup before an interface applies current policy.
use super::{
    admission_service::bounded, command_key_digest, AgentCommand, AgentCommandService,
    CommandAdmissionError, CommandLedgerError,
};
use crate::{
    resource_access::{AccessPurpose, CallerContext, ResourceCapability, ResourceTarget},
    AgentId,
};
use std::time::SystemTime;

impl AgentCommandService {
    /// Read an admitted message under the current binding's exact key scope.
    /// The interface must compare its complete reconstructed request before replay.
    pub async fn retained_binding_message(
        &self,
        context: &CallerContext,
        credential: &str,
        agent_id: &AgentId,
        key: &str,
    ) -> Result<Option<(AgentCommand, super::CommandReceiptView)>, CommandAdmissionError> {
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let capability = ResourceCapability::Interact;
        let authorized = bounded(self.access.authorize(
            context,
            Some(credential),
            &target,
            capability,
            AccessPurpose::Mutation {
                started_at: SystemTime::now(),
            },
        ))
        .await??;
        let proof = authorized
            .binding_authority()
            .ok_or(CommandLedgerError::NotFound)?;
        let authority = proof.command_authority();
        let scope =
            authority.idempotency_scope(agent_id, crate::sandbox::AgentOperationKind::SendMessage);
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store
                    .get_admitted_command(&scope, &command_key_digest(key)?),
            )
            .await;
        self.fence(&authorized, &target, capability).await?;
        let Some(record) = result?? else {
            return Ok(None);
        };
        record.validate()?;
        proof.require_command(&record.receipt.envelope)?;
        let receipt = super::CommandStatusView::from_record(&record).receipt;
        Ok(Some((record.receipt.envelope.command, receipt)))
    }
}
