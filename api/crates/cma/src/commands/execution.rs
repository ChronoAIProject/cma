use super::channel_control_execution::is_channel_control;
use super::channel_execution::is_channel;
use std::sync::Arc;

use layer2_core::now_rfc3339;
use layer2_core::resource_access::{
    AccessError, ResourceAccessError, ResourceAccessService, ResourceAccessStoreError,
};
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::{
    AgentContextService, AgentOperationError, AgentOperationService, SandboxStatus,
};

pub(crate) struct ResourceCommandExecution {
    pub channel_controls: Option<Arc<ChannelControlExecutionService>>,
    pub channel_revival: Option<Arc<super::ChannelCommandRevival>>,
    pub channel: Option<Arc<ChannelAgentCommandService>>,
    pub invokes: Option<Arc<layer2_core::bridge::invoke::BridgeInvokeService>>,
    pub authority: Arc<dyn AgentCommandAuthorizer>,
    pub access: Arc<ResourceAccessService>,
    pub store: Arc<dyn AgentCommandWorkerStore>,
    pub operations: Arc<AgentOperationService>,
    pub context: Arc<AgentContextService>,
}

impl ResourceCommandExecution {
    async fn checked(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<CommandExecutionAuthorization, CommandReadiness> {
        command.validate().map_err(|_| CommandReadiness::Retry)?;
        let authority = self
            .authority
            .authorize_resource_command(&self.access, &command.receipt.envelope)
            .await
            .map_err(authority_readiness)?;
        self.inspect_target(&authority, command).await?;
        Ok(authority)
    }

    async fn inspect_target(
        &self,
        authority: &CommandExecutionAuthorization,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandReadiness> {
        let lineage = authority
            .current_snapshot()
            .await
            .map_err(|error| authority_readiness(error.into()))?
            .lineage;
        let agent = lineage.agent.ok_or(CommandReadiness::Retry)?;
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
        if !matches!(
            command.receipt.envelope.command,
            AgentCommand::Message { .. }
        ) && (command.target_turn.is_none()
            || command.target_turn != agent.conversation.active_turn)
        {
            return Err(CommandReadiness::Reject(
                CommandFailureCode::TargetTurnChanged,
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl AgentCommandExecution for ResourceCommandExecution {
    async fn release_held(&self, command: &AdmittedAgentCommand) {
        if is_channel_control(command) {
            if let Some(service) = &self.channel_controls {
                let _ = service.release_held(command, &self.operations).await;
            }
            return;
        }
        if !is_channel(command) {
            return;
        }
        let (Some(channel), Some(invokes)) = (&self.channel, &self.invokes) else {
            return;
        };
        if command.queue_expired(&now_rfc3339()) {
            let _ = channel
                .refuse_execution_hold(command, ChannelHoldRefusalReason::QueueTimeout)
                .await;
            return;
        }
        match channel.authorize_execution(invokes, command).await {
            Ok(authority) => {
                let _ = channel.release_execution_hold(&authority, command).await;
            }
            Err(ChannelCommandAdmissionError::Invoke(
                layer2_core::bridge::invoke::BridgeInvokeError::Revoked,
            )) => {
                let _ = channel
                    .refuse_execution_hold(command, ChannelHoldRefusalReason::GrantRevoked)
                    .await;
            }
            Err(ChannelCommandAdmissionError::Invoke(
                layer2_core::bridge::invoke::BridgeInvokeError::NotFound,
            )) => {
                // The authorization lookup does not identify which retained
                // source disappeared. The refusal store independently proves
                // each exact absence alongside the unchanged command write.
                for reason in [
                    ChannelHoldRefusalReason::TargetMissing,
                    ChannelHoldRefusalReason::BridgeMissing,
                    ChannelHoldRefusalReason::GrantMissing,
                ] {
                    if channel.refuse_execution_hold(command, reason).await.is_ok() {
                        break;
                    }
                }
            }
            Err(_) => {}
        }
    }

    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        if is_channel_control(command) {
            return self.check_channel_control(command).await;
        }
        if super::trigger_execution::is_trigger(command) {
            return self
                .operations
                .trigger_ready_for_claim(command)
                .await
                .map(|_| CommandReadiness::Ready)
                .unwrap_or_else(|error| match classify(error) {
                    CommandAttempt::Reject(code) => CommandReadiness::Reject(code),
                    _ => CommandReadiness::Retry,
                });
        }
        if is_channel(command) {
            return self
                .channel_checked(command)
                .await
                .map(|_| CommandReadiness::Ready)
                .unwrap_or_else(|readiness| readiness);
        }
        match self.checked(command).await {
            Ok(_) => CommandReadiness::Ready,
            Err(readiness) => readiness,
        }
    }

    async fn prepare_queued(&self, command: &AdmittedAgentCommand) {
        if is_channel_control(command) {
            return;
        }
        if super::trigger_execution::is_trigger(command) {
            return;
        }
        if is_channel(command) {
            if let Some(revival) = &self.channel_revival {
                if let Ok(authority) = self.channel_checked(command).await {
                    revival.prepare(command, &authority).await;
                }
            }
            return;
        }
        if !matches!(
            command.receipt.envelope.command,
            AgentCommand::Message { .. }
        ) {
            return;
        }
        let Ok(_) = self.checked(command).await else {
            return;
        };
        // Provider and policy IO can overlap another replica's claim. Recheck
        // the authoritative record immediately before entering lifecycle work.
        let Ok(Some(current)) = self
            .store
            .get_command(&command.receipt.envelope.command_id)
            .await
        else {
            return;
        };
        if current != *command
            || current.validate().is_err()
            || !current.awaiting_claim(&now_rfc3339())
            || current.queue_expired(&now_rfc3339())
        {
            return;
        }
        let Ok(authority) = self
            .authority
            .authorize_command_revival(&self.access, &current)
            .await
        else {
            return;
        };
        let _ = self.context.prepare_command_input(authority).await;
    }

    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        if is_channel_control(command) {
            return self.execute_channel_control(command).await;
        }
        if super::trigger_execution::is_trigger(command) {
            return match self.operations.execute_trigger_claim(command).await {
                Ok(_) => CommandAttempt::Finished,
                Err(error) => classify(error),
            };
        }
        if is_channel(command) {
            return self.execute_channel(command).await;
        }
        let authority = match self.checked(command).await {
            Ok(authority) => authority,
            Err(CommandReadiness::Reject(code)) => return CommandAttempt::Reject(code),
            Err(_) => return CommandAttempt::Retry,
        };
        // Renewal changes the stored expiry while provider IO is pending. Use
        // that current lease, retaining the exact original claim and receipt.
        let Ok(Some(current)) = self
            .store
            .get_command(&command.receipt.envelope.command_id)
            .await
        else {
            return CommandAttempt::Retry;
        };
        if current.validate().is_err()
            || current.receipt != command.receipt
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
            .execute_resource_claim(&authority, &current)
            .await
        {
            Ok(_) => CommandAttempt::Finished,
            Err(AgentOperationError::Conflict) => {
                match self.inspect_target(&authority, &current).await {
                    Err(CommandReadiness::Reject(code)) => CommandAttempt::Reject(code),
                    _ => CommandAttempt::Retry,
                }
            }
            Err(error) => classify(error),
        }
    }
}

pub(super) fn classify(error: AgentOperationError) -> CommandAttempt {
    use AgentOperationError::*;
    use CommandFailureCode as Failure;
    // The driver rereads binding state before acting on any classification.
    // Errors after an unknown commit therefore only lead to reconciliation.
    match error {
        Forbidden => CommandAttempt::Reject(Failure::AuthorityRevoked),
        NotFound => CommandAttempt::Reject(Failure::AgentUnavailable),
        StatusConflict { status } if !status.is_live() => {
            CommandAttempt::Reject(Failure::AgentUnavailable)
        }
        NoActiveTurn | PreviousTurnConflict | ApprovalNotPending { .. } => {
            CommandAttempt::Reject(Failure::TargetTurnChanged)
        }
        CapabilityUnsupported { .. }
        | ContentUnsupported
        | InvalidRequest(_)
        | LogCapacityExceeded => CommandAttempt::Reject(Failure::ExecutionFailed),
        _ => CommandAttempt::Retry,
    }
}

fn authority_readiness(error: ResourceCommandAuthorizationError) -> CommandReadiness {
    match error {
        ResourceCommandAuthorizationError::Resource(ResourceAccessError::Access(
            AccessError::NotFound,
        )) => CommandReadiness::Reject(CommandFailureCode::AgentUnavailable),
        ResourceCommandAuthorizationError::Custody(CommandAuthorizationError::Revoked)
        | ResourceCommandAuthorizationError::Resource(ResourceAccessError::Access(_)) => {
            CommandReadiness::Reject(CommandFailureCode::AuthorityRevoked)
        }
        ResourceCommandAuthorizationError::Custody(CommandAuthorizationError::Unavailable) => {
            CommandReadiness::Reject(CommandFailureCode::AuthorityUnavailable)
        }
        ResourceCommandAuthorizationError::Resource(ResourceAccessError::Store(
            ResourceAccessStoreError::NotFound,
        )) => CommandReadiness::Reject(CommandFailureCode::AgentUnavailable),
        _ => CommandReadiness::Retry,
    }
}
