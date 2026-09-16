use super::execution::ResourceCommandExecution;
use layer2_core::bridge::channel_control::ChannelControlError;
use layer2_core::sandbox::agent_io::*;

pub(super) fn is_channel_control(command: &AdmittedAgentCommand) -> bool {
    super::channel_execution::is_channel(command)
        && matches!(
            command.receipt.envelope.command,
            AgentCommand::Steer { .. } | AgentCommand::Interrupt
        )
}
impl ResourceCommandExecution {
    pub(super) async fn check_channel_control(
        &self,
        command: &AdmittedAgentCommand,
    ) -> CommandReadiness {
        let Some(service) = &self.channel_controls else {
            return CommandReadiness::Retry;
        };
        match service.ready_for_claim(command, &self.operations).await {
            Ok(()) => CommandReadiness::Ready,
            Err(error) => readiness(error),
        }
    }
    pub(super) async fn execute_channel_control(
        &self,
        command: &AdmittedAgentCommand,
    ) -> CommandAttempt {
        let Some(service) = &self.channel_controls else {
            return CommandAttempt::Retry;
        };
        let authority = match service.authorize(command).await {
            Ok(authority) => authority,
            Err(error) => {
                return match readiness(error) {
                    CommandReadiness::Reject(code) => CommandAttempt::Reject(code),
                    _ => CommandAttempt::Retry,
                }
            }
        };
        match self
            .operations
            .execute_channel_control_claim(&authority, command)
            .await
        {
            Ok(_) => CommandAttempt::Finished,
            Err(error) => super::execution::classify(error),
        }
    }
}
fn readiness(error: ChannelControlError) -> CommandReadiness {
    match error {
        ChannelControlError::SourceUnavailable => {
            CommandReadiness::Reject(CommandFailureCode::TargetTurnChanged)
        }
        ChannelControlError::AuthorityUnavailable => {
            CommandReadiness::Reject(CommandFailureCode::AuthorityRevoked)
        }
        ChannelControlError::CapabilityUnsupported | ChannelControlError::Invalid => {
            CommandReadiness::Reject(CommandFailureCode::ExecutionFailed)
        }
        _ => CommandReadiness::Retry,
    }
}
