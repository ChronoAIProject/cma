use super::*;
use crate::sandbox::agent_io::AgentCommand;
use crate::sandbox::{
    AgentMessageOutcome, AgentOperationError, AgentOperationService, AgentStatus,
};

impl AgentOperationService {
    /// Dedicated Channel controls preserve the source turn and never fabricate
    /// a human Caller or permit approval resolution.
    pub async fn execute_channel_control_claim(
        &self,
        authority: &ChannelControlExecutionAuthorization,
        command: &AdmittedAgentCommand,
    ) -> Result<AgentMessageOutcome, AgentOperationError> {
        authority
            .fence()
            .require_dispatch(command)
            .map_err(operation_error)?;
        let claim = command
            .work
            .lease
            .as_ref()
            .ok_or(AgentOperationError::Conflict)?
            .claim
            .clone();
        if command.work.dispatch.is_some() {
            return Err(AgentOperationError::Conflict);
        }
        for _ in 0..super::super::super::agent_operation_service::MAX_OPERATION_COMMIT_RETRIES {
            let current_authority = authority.refreshed().await.map_err(operation_error)?;
            let fence = current_authority.fence();
            fence
                .require_dispatch(&fence.snapshot().admitted.command)
                .map_err(operation_error)?;
            if fence.snapshot().admitted.command.work.dispatch.is_some() {
                return Err(AgentOperationError::Conflict);
            }
            let current = fence
                .proof()
                .resources()
                .lineage
                .agent
                .clone()
                .ok_or(AgentOperationError::NotFound)?;
            let service = Self {
                command_claim: Some(claim.clone()),
                execution_authority: None,
                channel_authority: None,
                channel_control_authority: Some(current_authority),
                trigger_authority: None,
                connections: self.connections.clone(),
                store: self.store.clone(),
                runtimes: self.runtimes.clone(),
            };
            self.require_first_message_dispatch_settled(&current)?;
            self.require_dispatch_available(&current)?;
            let (parent, queues) = self.parent_admission(&current).await?;
            if queues {
                return Err(AgentOperationError::Conflict);
            }
            let prepared = match &command.receipt.envelope.command {
                AgentCommand::Steer { input } => {
                    if current.status != AgentStatus::Working {
                        return Err(AgentOperationError::StatusConflict {
                            status: current.status,
                        });
                    }
                    let text = input.clone().into_text()?;
                    self.validate_message(&text)?;
                    service.fence_steer(current, text, parent).await
                }
                AgentCommand::Interrupt => service.fence_interrupt(current, parent).await,
                _ => return Err(AgentOperationError::Forbidden),
            };
            match prepared {
                Ok(prepared) => {
                    return service
                        .dispatch_prepared(prepared)
                        .await
                        .map(AgentMessageOutcome::Dispatched)
                }
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }
}

pub(super) fn operation_error(error: Error) -> AgentOperationError {
    match error {
        Error::SourceUnavailable => AgentOperationError::PreviousTurnConflict,
        Error::AuthorityUnavailable => AgentOperationError::Forbidden,
        Error::CapabilityUnsupported => {
            AgentOperationError::InvalidRequest("Channel control capability is unavailable".into())
        }
        Error::Conflict => AgentOperationError::Conflict,
        Error::Corrupt => {
            AgentOperationError::Corrupt("Channel control evidence is corrupt".into())
        }
        Error::Invalid => AgentOperationError::Forbidden,
        Error::Unavailable => {
            AgentOperationError::Unavailable("Channel control authority is unavailable".into())
        }
    }
}
