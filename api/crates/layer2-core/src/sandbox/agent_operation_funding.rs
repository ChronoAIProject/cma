use super::super::agent_io::{AdmittedAgentCommand, AgentCommand};
use super::super::agent_operation_dispatch::PreparedAgentOperation;
use super::super::model_credit::{Effect, FundingError};
use super::*;
use layer3_adapter::{AdapterError, ConversationInputGuard, ConversationSendOutcome};

impl AgentOperationService {
    pub fn with_model_funding(&self, funding: Option<Arc<AgentTurnFunding>>) -> Arc<Self> {
        Arc::new(Self {
            model_funding: funding,
            ..self.clone()
        })
    }

    pub(in crate::sandbox) fn funded_claim(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<Option<FundedAgentMessage>, AgentOperationError> {
        if !self.model_credit_required
            || !matches!(
                command.receipt.envelope.command,
                AgentCommand::Message { .. }
            )
        {
            return Ok(None);
        }
        self.model_funding
            .as_ref()
            .ok_or(AgentOperationError::ModelFundingUnsupported)?;
        Ok(Some(FundedAgentMessage::admitted(command)?))
    }

    pub(in crate::sandbox) async fn check_model_funding(
        &self,
        agent: &Agent,
    ) -> Result<(), AgentOperationError> {
        self.require_model_credit()?;
        if let Some(message) = &self.funded_message {
            let funding = self
                .model_funding
                .as_ref()
                .ok_or(AgentOperationError::ModelFundingUnsupported)?;
            let sandbox = self.funding_sandbox(agent).await?;
            funding.check_message(message, agent, &sandbox).await?;
        }
        Ok(())
    }

    async fn funding_sandbox(
        &self,
        agent: &Agent,
    ) -> Result<super::super::Sandbox, AgentOperationError> {
        SandboxStore::get_sandbox(self.store.as_ref(), &agent.sandbox_id)
            .await
            .map_err(operation_store_error)?
            .filter(|sandbox| sandbox.owner == agent.owner)
            .ok_or(AgentOperationError::NotFound)
    }

    pub(in crate::sandbox) async fn send_funded(
        &self,
        prepared: &PreparedAgentOperation,
    ) -> ConversationSendOutcome {
        let result = async {
            let funding = self
                .model_funding
                .as_ref()
                .ok_or(FundingError::Unsupported)?;
            let message = self
                .funded_message
                .as_ref()
                .ok_or(FundingError::Unsupported)?;
            let agent = self
                .store
                .get_agent(&prepared.target.agent_id)
                .await
                .map_err(|_| FundingError::Unavailable)?
                .ok_or(FundingError::BindingChanged)?;
            if agent.conversation.runtime_generation != Some(prepared.target.runtime_generation)
                || agent
                    .conversation
                    .operation
                    .as_ref()
                    .is_none_or(|operation| {
                        operation.sequence != prepared.operation_sequence
                            || operation.state != AgentOperationDelivery::DispatchFenced
                    })
            {
                return Err(FundingError::BindingChanged);
            }
            let sandbox = self
                .funding_sandbox(&agent)
                .await
                .map_err(|_| FundingError::Unavailable)?;
            if let Some(authority) = &self.execution_authority {
                super::super::agent_io::execution_dispatch::CommandDispatchGuard {
                    authority: authority.clone(),
                    target: prepared.target.clone(),
                    operation_sequence: prepared.operation_sequence,
                    turn_sequence: prepared.turn_sequence,
                }
                .check()
                .await
                .map_err(|_| FundingError::BindingChanged)?;
            }
            funding
                .dispatch_message(message, &agent, &sandbox, prepared.target.clone())
                .await
        }
        .await;
        match result {
            Ok(Effect::Finished { .. }) => ConversationSendOutcome::WriteConfirmed,
            Ok(Effect::NotStarted) => {
                ConversationSendOutcome::PreWriteFailure(AdapterError::PromptNotWritten(
                    "the model gateway did not admit the funded request".into(),
                ))
            }
            Err(error) => self.resolve_failed_funded_send(error.to_string()).await,
        }
    }

    pub(in crate::sandbox) async fn resolve_failed_funded_send(
        &self,
        reason: String,
    ) -> ConversationSendOutcome {
        let outcome = match (&self.model_funding, &self.funded_message) {
            (Some(funding), Some(message)) => funding.resolve_failed_dispatch(message).await,
            _ => Err(FundingError::ReconciliationRequired),
        };
        match outcome {
            Ok(Effect::Finished { .. }) => ConversationSendOutcome::WriteConfirmed,
            Ok(Effect::NotStarted) => {
                ConversationSendOutcome::PreWriteFailure(AdapterError::PromptNotWritten(reason))
            }
            Err(_) => ConversationSendOutcome::Ambiguous(AdapterError::Unavailable(
                FundingError::ReconciliationRequired.to_string(),
            )),
        }
    }
}
