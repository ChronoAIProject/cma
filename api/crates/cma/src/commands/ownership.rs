//! The durable command ledger routes accepted commands without replaying input.
use layer2_core::agent_ownership::OwnershipCoordinator;
use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, AgentCommandExecution, CommandAttempt, CommandPreparationError,
    CommandReadiness,
};
use std::sync::Arc;

pub(crate) struct OwnedCommandExecution {
    pub inner: Arc<dyn AgentCommandExecution>,
    pub ownership: Arc<OwnershipCoordinator>,
}

#[async_trait::async_trait]
impl AgentCommandExecution for OwnedCommandExecution {
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        let readiness = self.inner.check_queued(command).await;
        if readiness != CommandReadiness::Ready {
            return readiness;
        }
        match self
            .ownership
            .acquire(&command.receipt.envelope.agent_id)
            .await
        {
            Ok(Some(_)) => CommandReadiness::Ready,
            _ => CommandReadiness::Retry,
        }
    }

    async fn prepare_queued(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        let authority = self
            .ownership
            .acquire(&command.receipt.envelope.agent_id)
            .await
            .map_err(|_| CommandPreparationError::AuthorityUnavailable)?
            .ok_or(CommandPreparationError::StateChanged)?;
        authority
            .run(self.inner.prepare_queued(command))
            .await
            .map_err(|_| CommandPreparationError::StateChanged)?
    }

    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        let Ok(Some(authority)) = self
            .ownership
            .acquire(&command.receipt.envelope.agent_id)
            .await
        else {
            return CommandAttempt::Retry;
        };
        authority
            .run(self.inner.execute(command))
            .await
            .unwrap_or(CommandAttempt::Retry)
    }
}
