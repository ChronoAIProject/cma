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
    pub source_guard: Option<Arc<dyn CommandSourceGuard>>,
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
        let mut authority = self
            .authority
            .authorize_resource_command(&self.access, &command.receipt.envelope)
            .await
            .map_err(authority_readiness)?;
        if command.receipt.envelope.continuation.is_some() {
            let source = self.source_guard.clone().ok_or(CommandReadiness::Reject(
                CommandFailureCode::AuthorityRevoked,
            ))?;
            authority = authority.with_source_guard(source);
        }
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
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        if matches!(command.receipt.envelope.actor, AgentActor::Channel { .. }) {
            return CommandReadiness::Reject(CommandFailureCode::AuthorityRevoked);
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
        match self.checked(command).await {
            Ok(_) => CommandReadiness::Ready,
            Err(readiness) => readiness,
        }
    }

    async fn prepare_queued(&self, command: &AdmittedAgentCommand) {
        if matches!(command.receipt.envelope.actor, AgentActor::Channel { .. }) {
            return;
        }
        if super::trigger_execution::is_trigger(command) {
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
        if matches!(command.receipt.envelope.actor, AgentActor::Channel { .. }) {
            return CommandAttempt::Reject(CommandFailureCode::AuthorityRevoked);
        }
        if super::trigger_execution::is_trigger(command) {
            return match self.operations.execute_trigger_claim(command).await {
                Ok(_) => CommandAttempt::Finished,
                Err(error) => classify(error),
            };
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
            CommandReadiness::Retry
        }
        ResourceCommandAuthorizationError::Resource(ResourceAccessError::Store(
            ResourceAccessStoreError::NotFound,
        )) => CommandReadiness::Reject(CommandFailureCode::AgentUnavailable),
        _ => CommandReadiness::Retry,
    }
}
