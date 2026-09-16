use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::SandboxStatus;

use super::execution::ResourceCommandExecution;

pub(super) fn is_channel(command: &AdmittedAgentCommand) -> bool {
    matches!(command.receipt.envelope.actor, AgentActor::Channel { .. })
        || command.receipt.envelope.source.kind == AgentSourceKind::Channel
}

impl ResourceCommandExecution {
    pub(super) async fn channel_checked(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<ChannelExecutionAuthorization, CommandReadiness> {
        let (Some(channel), Some(invokes)) = (&self.channel, &self.invokes) else {
            return Err(CommandReadiness::Retry);
        };
        let authority = channel
            .authorize_execution(invokes, command)
            .await
            .map_err(readiness)?;
        authority
            .fence()
            .require_dispatch(command)
            .map_err(readiness)?;
        let fence = authority.fence();
        let lineage = &fence.proof().resources().lineage;
        let agent = lineage.agent.as_ref().ok_or(CommandReadiness::Retry)?;
        if !agent.status.is_live()
            || agent.requested_stop_reason.is_some()
            || matches!(
                lineage.sandbox.status,
                SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired
            )
        {
            return Err(CommandReadiness::Reject(
                CommandFailureCode::AgentUnavailable,
            ));
        }
        Ok(authority)
    }

    pub(super) async fn execute_channel(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        let authority = match self.channel_checked(command).await {
            Ok(authority) => authority,
            Err(CommandReadiness::Reject(code)) => return CommandAttempt::Reject(code),
            Err(_) => return CommandAttempt::Retry,
        };
        // Lease renewal may have advanced its expiry during provider IO. Reuse
        // the current lease, but never substitute a new claim or receipt.
        let Ok(Some(current)) = self
            .store
            .get_command(&command.receipt.envelope.command_id)
            .await
        else {
            return CommandAttempt::Retry;
        };
        if current.receipt != command.receipt
            || current.key_sha256 != command.key_sha256
            || current.target_turn != command.target_turn
            || current.work.lease.as_ref().map(|lease| &lease.claim)
                != command.work.lease.as_ref().map(|lease| &lease.claim)
            || current.work.dispatch.is_some()
            || current.status.is_terminal()
        {
            return CommandAttempt::Retry;
        }
        match self
            .operations
            .execute_channel_claim(&authority, &current)
            .await
        {
            Ok(_) => CommandAttempt::Finished,
            Err(error) => super::execution::classify(error),
        }
    }
}

fn readiness(error: ChannelCommandAdmissionError) -> CommandReadiness {
    use layer2_core::bridge::invoke::BridgeInvokeError;
    match error {
        ChannelCommandAdmissionError::Invoke(BridgeInvokeError::Revoked) => {
            CommandReadiness::Reject(CommandFailureCode::AuthorityRevoked)
        }
        ChannelCommandAdmissionError::Invoke(BridgeInvokeError::NotFound) => {
            CommandReadiness::Reject(CommandFailureCode::AgentUnavailable)
        }
        _ => CommandReadiness::Retry,
    }
}
