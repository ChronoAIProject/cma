use super::*;
use crate::{
    agent_ownership::OwnershipCoordinator,
    sandbox::{Agent, FirstMessageDelivery},
};

impl AgentCommandService {
    pub fn with_ownership(
        self: Arc<Self>,
        ownership: Option<Arc<OwnershipCoordinator>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            ownership,
            ..(*self).clone()
        })
    }

    /// Ordinary queued cancellation changes only the ledger. The original
    /// queued Trigger first message also projects terminal Agent history.
    pub(in crate::sandbox::agent_io) async fn cancel_with_projection_owner(
        &self,
        agent: &Agent,
        request: QueuedCancellationReservation,
    ) -> Result<QueuedCancellationOutcome, QueuedCancellationError> {
        let cancel = || self.store.cancel_queued_response(request.clone());
        let Some(ownership) = &self.ownership else {
            return Ok(cancel().await?);
        };
        let requires_owner = agent
            .conversation
            .first_message
            .as_ref()
            .is_some_and(|first| {
                first.state == FirstMessageDelivery::Queued
                    && first.common_binding.as_ref().is_some_and(|binding| {
                        let receipt = binding.receipt();
                        receipt.envelope.command_id == request.target.source_command_id
                            && receipt.response_id.as_ref() == Some(&request.target.response_id)
                    })
            });
        if !requires_owner
            || crate::agent_ownership::execution_owner()
                .is_some_and(|token| token.agent_id == agent.id)
        {
            // A relayed request carries the one epoch verified by the receiving
            // owner. The store fences that same epoch in the write transaction.
            return Ok(cancel().await?);
        }
        let authority = ownership
            .acquire(&agent.id)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?
            .ok_or(QueuedCancellationError::OwnerRequired)?;
        Ok(authority
            .run(cancel())
            .await
            .map_err(|_| CommandLedgerError::Unavailable)??)
    }
}
