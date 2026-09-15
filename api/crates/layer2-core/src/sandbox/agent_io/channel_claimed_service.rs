use crate::sandbox::{
    AgentMessageOutcome, AgentOperationError, AgentOperationService, AgentStatus,
};
use crate::{now_rfc3339, RuntimeCapability};

use super::{AdmittedAgentCommand, AgentCommand, ChannelExecutionAuthorization};

impl AgentOperationService {
    /// Execute exactly the claimed Channel message through the common
    /// conversation transaction and ordered writer, without a human Caller.
    pub async fn execute_channel_claim(
        &self,
        authority: &ChannelExecutionAuthorization,
        command: &AdmittedAgentCommand,
    ) -> Result<AgentMessageOutcome, AgentOperationError> {
        command
            .validate()
            .map_err(|_| AgentOperationError::Corrupt("command ledger is invalid".into()))?;
        let fence = authority.fence();
        fence
            .require_dispatch(command)
            .map_err(|_| AgentOperationError::Forbidden)?;
        let claim = &command
            .work
            .lease
            .as_ref()
            .ok_or(AgentOperationError::Conflict)?
            .claim;
        command
            .require_claim(claim, &now_rfc3339())
            .map_err(|_| AgentOperationError::Conflict)?;
        if command.work.dispatch.is_some() {
            return Err(AgentOperationError::Conflict);
        }
        let AgentCommand::Message {
            input, metadata, ..
        } = &command.receipt.envelope.command
        else {
            return Err(AgentOperationError::Forbidden);
        };
        let text = input.clone().into_text()?;
        self.validate_message(&text)?;
        let current = fence
            .proof()
            .resources()
            .lineage
            .agent
            .clone()
            .ok_or(AgentOperationError::NotFound)?;
        if current.status != AgentStatus::Ready || current.requested_stop_reason.is_some() {
            return Err(AgentOperationError::StatusConflict {
                status: current.status,
            });
        }
        current
            .capabilities
            .require([RuntimeCapability::Conversation])
            .map_err(|capability| AgentOperationError::CapabilityUnsupported { capability })?;
        self.require_first_message_settled(&current)?;
        self.require_dispatch_available(&current)?;
        self.require_agent_connections(&current).await?;
        let (parent, queues) = self.parent_admission(&current).await?;
        if queues {
            return Err(AgentOperationError::Conflict);
        }
        let service = Self {
            command_claim: Some(claim.clone()),
            execution_authority: None,
            channel_authority: Some(authority.clone()),
            connections: self.connections.clone(),
            store: self.store.clone(),
            runtimes: self.runtimes.clone(),
        };
        let prepared = service
            .fence_message(current, text, parent, metadata.clone())
            .await?;
        service
            .dispatch_prepared(prepared)
            .await
            .map(AgentMessageOutcome::Dispatched)
    }
}
