use layer2_core::now_rfc3339;
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::{AgentContextError, SandboxLifecycleCommandError};

use super::execution::{authority_readiness, authorization_error_kind, ResourceCommandExecution};

impl ResourceCommandExecution {
    pub(super) async fn prepare(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        use CommandPreparationError as Error;
        if matches!(command.receipt.envelope.actor, AgentActor::Channel { .. }) {
            return Err(Error::Rejected(CommandFailureCode::AuthorityRevoked));
        }
        if super::trigger_execution::is_first_message(command)
            || !matches!(
                command.receipt.envelope.command,
                AgentCommand::Message { .. }
            )
        {
            return Ok(());
        }
        self.checked(command).await.map_err(readiness_error)?;
        // Provider and policy IO can overlap another replica's claim. Recheck
        // the exact durable command before entering lifecycle work.
        let current = observe_command_preparation(
            command,
            "command_refresh",
            self.store.get_command(&command.receipt.envelope.command_id),
            |error| match error {
                CommandLedgerError::Corrupt => "command_corrupt",
                _ => "command_store_unavailable",
            },
        )
        .await
        .map_err(|error| match error {
            CommandLedgerError::Corrupt | CommandLedgerError::Invalid => {
                Error::Rejected(CommandFailureCode::ExecutionFailed)
            }
            _ => Error::DependencyUnavailable,
        })?
        .ok_or(Error::StateChanged)?;
        current
            .validate()
            .map_err(|_| Error::Rejected(CommandFailureCode::ExecutionFailed))?;
        if current != *command
            || !current.awaiting_claim(&now_rfc3339())
            || current.queue_expired(&now_rfc3339())
        {
            return Err(Error::StateChanged);
        }
        let authority = observe_command_preparation(
            &current,
            "revival_authorization",
            self.authority
                .authorize_command_revival(&self.access, &current),
            authorization_error_kind,
        )
        .await
        .map_err(|error| readiness_error(authority_readiness(error)))?;
        self.context
            .prepare_command_input(authority)
            .await
            .map_err(context_error)
    }
}

fn readiness_error(readiness: CommandReadiness) -> CommandPreparationError {
    match readiness {
        CommandReadiness::Reject(code) => CommandPreparationError::Rejected(code),
        CommandReadiness::WaitingForActiveTurn => CommandPreparationError::StateChanged,
        _ => CommandPreparationError::AuthorityUnavailable,
    }
}

fn context_error(error: AgentContextError) -> CommandPreparationError {
    use CommandFailureCode as Failure;
    use CommandPreparationError as Error;
    use SandboxLifecycleCommandError as Lifecycle;
    match error {
        AgentContextError::Forbidden | AgentContextError::Lifecycle(Lifecycle::Forbidden) => {
            Error::Rejected(Failure::AuthorityRevoked)
        }
        AgentContextError::NotReady | AgentContextError::Lifecycle(Lifecycle::NotFound) => {
            Error::Rejected(Failure::AgentUnavailable)
        }
        AgentContextError::Lifecycle(Lifecycle::CredentialSetupRequired) => {
            Error::Rejected(Failure::CredentialSetupRequired)
        }
        AgentContextError::Lifecycle(Lifecycle::ConnectionsRequired { .. }) => {
            Error::Rejected(Failure::ConnectionsRequired)
        }
        AgentContextError::Lifecycle(Lifecycle::Corrupt(_)) => {
            Error::Rejected(Failure::ExecutionFailed)
        }
        AgentContextError::Lifecycle(Lifecycle::Budget(reason)) => {
            Error::Rejected(Failure::from_budget(reason))
        }
        AgentContextError::Lifecycle(
            Lifecycle::SandboxBusy { .. }
            | Lifecycle::SandboxNotActive { .. }
            | Lifecycle::SandboxNotPaused { .. },
        ) => Error::StateChanged,
        _ => Error::DependencyUnavailable,
    }
}
