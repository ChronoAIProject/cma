//! One owner scope covers the whole durable command attempt, not just input.
use layer2_core::agent_ownership::{execution_authority, Authority, OwnershipCoordinator};
use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, AgentCommandExecution, CommandAttempt, CommandPreparationError,
    CommandReadiness,
};
use std::sync::Arc;

pub(crate) struct OwnedCommandExecution {
    pub inner: Arc<dyn AgentCommandExecution>,
    pub ownership: Arc<OwnershipCoordinator>,
}

impl OwnedCommandExecution {
    fn current(&self, command: &AdmittedAgentCommand) -> bool {
        self.ownership.is_ready()
            && execution_authority().is_some_and(|authority| {
                authority.is_current()
                    && authority.token.agent_id == command.receipt.envelope.agent_id
                    && authority.token.instance_id == self.ownership.instance_id
            })
    }
}

#[async_trait::async_trait]
impl AgentCommandExecution for OwnedCommandExecution {
    async fn attempt_authority(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<Option<Arc<Authority>>, CommandPreparationError> {
        // None is reserved for the uncoordinated composition. Denied ownership
        // always defers the complete attempt before any command/Agent mutation.
        self.ownership
            .acquire(&command.receipt.envelope.agent_id)
            .await
            .map_err(|_| CommandPreparationError::AuthorityUnavailable)?
            .map(Some)
            .ok_or(CommandPreparationError::AuthorityUnavailable)
    }

    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        if !self.current(command) {
            return CommandReadiness::Retry;
        }
        self.inner.check_queued(command).await
    }

    async fn prepare_queued(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        if !self.current(command) {
            return Err(CommandPreparationError::StateChanged);
        }
        self.inner.prepare_queued(command).await
    }

    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        if !self.current(command) {
            return CommandAttempt::Retry;
        }
        self.inner.execute(command).await
    }
}
