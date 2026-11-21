use super::*;
use crate::{
    agent_ownership::OwnershipCoordinator,
    sandbox::{task_experience::TaskExperiencePolicy, Agent, FirstMessageDelivery},
};

impl AgentCommandService {
    pub fn with_task_experience(self: Arc<Self>, policy: Arc<TaskExperiencePolicy>) -> Arc<Self> {
        Arc::new(Self {
            task_experience: policy,
            ..(*self).clone()
        })
    }

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
    ) -> Result<QueuedCancellationOutcome, CommandLedgerError> {
        let Some(ownership) = &self.ownership else {
            return self.store.cancel_queued_response(request).await;
        };
        let requires_owner = agent
            .conversation
            .first_message
            .as_ref()
            .is_some_and(|first| {
                first.state == FirstMessageDelivery::Queued
                    && first.common_binding.as_ref().is_some_and(|binding| {
                        binding.receipt().envelope.command_id == request.target.source_command_id
                    })
            });
        if !requires_owner {
            return self.store.cancel_queued_response(request).await;
        }
        let authority = ownership
            .acquire(&agent.id)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?
            .ok_or(CommandLedgerError::Unavailable)?;
        authority
            .run(self.store.cancel_queued_response(request))
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?
    }
}
