use std::sync::Arc;

use layer2_core::resource_access::{
    AccessError, ResourceAccessError, ResourceAccessService, ResourceAccessStoreError,
};
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::{
    AgentContextService, AgentOperationError, AgentOperationService, AgentStatus, SandboxStatus,
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
    pub(super) async fn checked(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<CommandExecutionAuthorization, CommandReadiness> {
        command.validate().map_err(|_| CommandReadiness::Retry)?;
        let mut authority = observe_command_preparation(
            command,
            "command_authorization",
            self.authority
                .authorize_resource_command(&self.access, &command.receipt.envelope),
            authorization_error_kind,
        )
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
            || command.predates_owner_restart(&agent)
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
        if agent.status != AgentStatus::Ready
            && matches!(
                command.receipt.envelope.command,
                AgentCommand::Message { .. }
            )
        {
            tracing::warn!(event = "command_readiness", boundary = "agent_readiness",
                command_id = %command.receipt.envelope.command_id,
                agent_id = %agent.id, sandbox_id = %lineage.sandbox.id,
                agent_status = ?agent.status, sandbox_status = ?lineage.sandbox.status,
                error_kind = "agent_not_ready", "Queued command is waiting for its Agent");
        }
        // Publication is durable work driven by the recovery worker. A queued
        // append can wait without taking another command claim or owner scope.
        if command.awaits_connection_publication(&agent, &lineage.sandbox) {
            return Err(CommandReadiness::WaitingForAttachment);
        }
        // Ordinary appends behind a live turn need neither an ownership claim
        // nor revival. The driver will re-read durable readiness after waiting.
        if lineage.sandbox.status == SandboxStatus::Active
            && agent.status == AgentStatus::Working
            && agent.conversation.active_turn.is_some()
            && command.receipt.envelope.is_ordinary_append()
        {
            return Err(CommandReadiness::WaitingForActiveTurn);
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
        if super::trigger_execution::is_first_message(command) {
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

    async fn prepare_queued(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        observe_command_preparation(command, "prepare_queued", self.prepare(command), |_| {
            "preparation_failed"
        })
        .await
    }

    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        if matches!(command.receipt.envelope.actor, AgentActor::Channel { .. }) {
            return CommandAttempt::Reject(CommandFailureCode::AuthorityRevoked);
        }
        if super::trigger_execution::is_first_message(command) {
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
        match observe_command_preparation(
            &current,
            "command_execution",
            self.operations.execute_resource_claim(&authority, &current),
            AgentOperationError::code,
        )
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
        RuntimeAttachmentUnavailable(_) => CommandAttempt::RetryAttachment,
        Budget(reason) => CommandAttempt::Reject(Failure::from_budget(reason)),
        SessionCapacityUnavailable => CommandAttempt::Reject(Failure::SessionCapacityUnavailable),
        Connection(layer2_core::sandbox::SandboxCredentialPrerequisiteError::SetupRequired) => {
            CommandAttempt::Reject(Failure::CredentialSetupRequired)
        }
        Connection(
            layer2_core::sandbox::SandboxCredentialPrerequisiteError::ConnectionsRequired {
                ..
            },
        ) => CommandAttempt::Reject(Failure::ConnectionsRequired),
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

pub(super) fn authority_readiness(error: ResourceCommandAuthorizationError) -> CommandReadiness {
    match error {
        ResourceCommandAuthorizationError::Resource(ResourceAccessError::Access(
            AccessError::NotFound,
        )) => CommandReadiness::Reject(CommandFailureCode::AgentUnavailable),
        ResourceCommandAuthorizationError::Resource(ResourceAccessError::Access(
            AccessError::RevalidationRequired,
        )) => CommandReadiness::Retry,
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

pub(super) fn authorization_error_kind(error: &ResourceCommandAuthorizationError) -> &'static str {
    match error {
        ResourceCommandAuthorizationError::Custody(CommandAuthorizationError::Revoked) => {
            "custody_revoked"
        }
        ResourceCommandAuthorizationError::Custody(CommandAuthorizationError::Unavailable) => {
            "custody_unavailable"
        }
        ResourceCommandAuthorizationError::Resource(error) => command_access_error_kind(error),
    }
}

#[cfg(test)]
mod session_capacity_tests {
    use super::*;

    #[test]
    fn session_capacity_refusal_is_terminal_before_runtime_dispatch() {
        assert!(matches!(
            classify(AgentOperationError::SessionCapacityUnavailable),
            CommandAttempt::Reject(CommandFailureCode::SessionCapacityUnavailable)
        ));
    }
}
