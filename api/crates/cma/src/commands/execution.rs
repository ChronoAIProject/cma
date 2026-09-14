use std::sync::Arc;

use layer2_core::resource_access::{
    AccessError, ResourceAccessError, ResourceAccessService, ResourceAccessStoreError,
    ResourceCapability, ResourceTarget,
};
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::{
    AgentContextService, AgentOperationError, AgentOperationService, SandboxStatus,
};
use layer2_core::{now_rfc3339, Caller};

pub(crate) struct PersonalCommandExecution {
    pub authority: Arc<dyn AgentCommandAuthorizer>,
    pub access: Arc<ResourceAccessService>,
    pub store: Arc<dyn AgentCommandWorkerStore>,
    pub operations: Arc<AgentOperationService>,
    pub context: Arc<AgentContextService>,
}

impl PersonalCommandExecution {
    async fn checked(&self, command: &AdmittedAgentCommand) -> Result<Caller, CommandReadiness> {
        command.validate().map_err(|_| CommandReadiness::Retry)?;
        let envelope = &command.receipt.envelope;
        let caller = self
            .authority
            .authorize_command(envelope)
            .await
            .map_err(|error| {
                CommandReadiness::Reject(match error {
                    CommandAuthorizationError::Revoked => CommandFailureCode::AuthorityRevoked,
                    CommandAuthorizationError::Unavailable => {
                        CommandFailureCode::AuthorityUnavailable
                    }
                })
            })?;
        if envelope.owner
            != (AgentOwner::Person {
                subject: caller.subject.clone(),
            })
            || !matches!(&envelope.actor,
                AgentActor::Human { subject } | AgentActor::Api { subject, delegation_id: None }
                if subject == &caller.subject)
        {
            return Err(CommandReadiness::Reject(
                CommandFailureCode::AuthorityRevoked,
            ));
        }
        self.inspect_target(&caller, command).await?;
        Ok(caller)
    }

    async fn inspect_target(
        &self,
        caller: &Caller,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandReadiness> {
        let authorized = self
            .access
            .authorize_personal(
                caller,
                &ResourceTarget::Agent {
                    agent_id: command.receipt.envelope.agent_id.clone(),
                },
                ResourceCapability::Interact,
            )
            .await
            .map_err(|error| match error {
                ResourceAccessError::Access(AccessError::NotFound)
                | ResourceAccessError::Store(ResourceAccessStoreError::NotFound) => {
                    CommandReadiness::Reject(CommandFailureCode::AgentUnavailable)
                }
                ResourceAccessError::Access(_) => {
                    CommandReadiness::Reject(CommandFailureCode::AuthorityRevoked)
                }
                _ => CommandReadiness::Retry,
            })?;
        let lineage = authorized.snapshot.lineage;
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
impl AgentCommandExecution for PersonalCommandExecution {
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        match self.checked(command).await {
            Ok(_) => CommandReadiness::Ready,
            Err(readiness) => readiness,
        }
    }

    async fn prepare_queued(&self, command: &AdmittedAgentCommand) {
        if !matches!(
            command.receipt.envelope.command,
            AgentCommand::Message { .. }
        ) {
            return;
        }
        let Ok(caller) = self.checked(command).await else {
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
        let _ = self
            .context
            .await_input_ready(&caller, &command.receipt.envelope.agent_id)
            .await;
    }

    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        let caller = match self.checked(command).await {
            Ok(caller) => caller,
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
        let authority = match self
            .access
            .authorize_personal_command_execution(&caller, &current.receipt.envelope)
            .await
        {
            Ok(authority) => authority,
            Err(ResourceAccessError::Access(_)) => {
                return CommandAttempt::Reject(CommandFailureCode::AuthorityRevoked)
            }
            Err(_) => return CommandAttempt::Retry,
        };
        match self
            .operations
            .execute_resource_claim(&authority, &current)
            .await
        {
            Ok(_) => CommandAttempt::Finished,
            Err(AgentOperationError::Conflict) => {
                match self.inspect_target(&caller, &current).await {
                    Err(CommandReadiness::Reject(code)) => CommandAttempt::Reject(code),
                    _ => CommandAttempt::Retry,
                }
            }
            Err(error) => classify(error),
        }
    }
}

fn classify(error: AgentOperationError) -> CommandAttempt {
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
