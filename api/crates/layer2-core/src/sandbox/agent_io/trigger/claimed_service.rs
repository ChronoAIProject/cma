use super::super::{AdmittedAgentCommand, AgentCommand};
use super::{is_bound_trigger_first, TriggerExecutionAuthorization};
use crate::sandbox::{
    AgentMessageOutcome, AgentOperationError, AgentOperationService, AgentStatus,
};
use crate::{now_rfc3339, RuntimeCapability};

impl AgentOperationService {
    /// Admit only a readiness check. Actual execution obtains a new live proof
    /// after the command worker atomically selects its replacement claim.
    pub async fn trigger_ready_for_claim(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), AgentOperationError> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        tokio::time::timeout_at(deadline, self.store.trigger_ready_for_claim(command))
            .await
            .map_err(|_| AgentOperationError::Conflict)?
            .map_err(super::super::super::agent_operation_service::operation_store_error)?;
        if tokio::time::Instant::now() >= deadline {
            return Err(AgentOperationError::Conflict);
        }
        Ok(())
    }

    pub async fn authorize_trigger_claim(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<TriggerExecutionAuthorization, AgentOperationError> {
        TriggerExecutionAuthorization::new(self.store.clone(), command).await
    }
    pub async fn execute_trigger_claim(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<AgentMessageOutcome, AgentOperationError> {
        let authority = self.authorize_trigger_claim(command).await?;
        let proof = authority.current_proof().await?;
        let current = proof.agent().clone();
        let claim = command
            .work
            .lease
            .as_ref()
            .ok_or(AgentOperationError::Conflict)?
            .claim
            .clone();
        command
            .require_claim(&claim, &now_rfc3339())
            .map_err(|_| AgentOperationError::Conflict)?;
        if command.work.dispatch.is_some()
            || current.status != AgentStatus::Ready
            || !is_bound_trigger_first(command, &current)
        {
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
        current
            .capabilities
            .require([RuntimeCapability::Conversation])
            .map_err(|capability| AgentOperationError::CapabilityUnsupported { capability })?;
        self.require_dispatch_available(&current)?;
        self.require_agent_connections(&current).await?;
        let (parent, queues) = self.parent_admission(&current).await?;
        if queues {
            return Err(AgentOperationError::Conflict);
        }
        let service = Self {
            settings_authority: None,
            command_claim: Some(claim),
            execution_authority: None,
            trigger_authority: Some(authority),
            instruction_policy: self.instruction_policy.clone(),
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
