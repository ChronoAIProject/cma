//! A new, authenticated input may interrupt the previous turn of its own
//! channel-created Agent. The input remains queued; the cancellation gets its
//! own durable Agent operation and an atomic receipt on that input's ledger row.

use super::{ChannelExecutionAuthorization, ChannelExecutionFence};
use crate::sandbox::agent_io::{AdmittedAgentCommand, AgentCommand, AgentResponseStatus};
use crate::sandbox::store::{AgentConversationMutation, SandboxStoreError};
use crate::sandbox::{
    Agent, AgentLogKind, AgentOperationDelivery, AgentOperationError, AgentOperationIntent,
    AgentOperationService, AgentStatus,
};
use crate::RuntimeCapability;

#[derive(Clone)]
pub(super) struct ChannelSupersession {
    command: AdmittedAgentCommand,
    turn_sequence: u64,
}

impl ChannelExecutionFence {
    pub fn superseding_command(&self) -> Option<&AdmittedAgentCommand> {
        self.supersession.as_ref().map(|value| &value.command)
    }

    /// Validate the exact cancellation mutation. A supersession authority can
    /// never start a turn, inject text, resolve approvals, or bind a command claim.
    pub fn validate_supersession(
        &self,
        current: &Agent,
        mutation: &AgentConversationMutation,
    ) -> Result<(), SandboxStoreError> {
        let invalid = || SandboxStoreError::Conflict;
        self.require_current().map_err(|_| invalid())?;
        let supersession = self.supersession.as_ref().ok_or_else(invalid)?;
        let expected = self
            .proof()
            .resources()
            .lineage
            .agent
            .as_ref()
            .ok_or_else(invalid)?;
        let operation = mutation
            .agent
            .conversation
            .operation
            .as_ref()
            .ok_or_else(invalid)?;
        if self.proof().grant().channel.is_none()
            || current != expected
            || mutation.command_claim.is_some()
            || mutation.admission_guard.is_none()
            || current.conversation.active_turn != Some(supersession.turn_sequence)
            || mutation.agent.conversation.active_turn != current.conversation.active_turn
            || mutation.agent.conversation.conversation_id != current.conversation.conversation_id
            || mutation.agent.conversation.runtime_generation
                != current.conversation.runtime_generation
            || operation.turn_sequence != supersession.turn_sequence
            || operation.runtime_generation != mutation.runtime_generation
            || operation.sequence != current.conversation.last_seq.saturating_add(1)
            || operation.intent != AgentOperationIntent::Interrupt
            || operation.state != AgentOperationDelivery::DispatchFenced
            || operation.dispatch_fenced_at != mutation.appended_at
            || mutation.agent_logs.iter().any(|log| {
                !matches!(
                    log,
                    AgentLogKind::OperationDeliveryChanged { .. }
                        | AgentLogKind::StatusChanged { .. }
                )
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl AgentOperationService {
    /// Best-effort supersession is retried by the queued-input worker until the
    /// durable cancellation is fenced or the previous turn has already finished.
    pub async fn interrupt_channel_for_input(
        &self,
        authority: &ChannelExecutionAuthorization,
        command: &AdmittedAgentCommand,
    ) -> Result<(), AgentOperationError> {
        let fence = authority.fence();
        fence
            .source()
            .require_result(command)
            .map_err(|_| AgentOperationError::Forbidden)?;
        fence
            .require_current()
            .map_err(|_| AgentOperationError::Forbidden)?;
        if fence.proof().grant().channel.is_none()
            || fence.source().inbox().envelope.conversation_type != "private"
            || !matches!(
                command.receipt.envelope.command,
                AgentCommand::Message { .. }
            )
            || !matches!(
                command.status,
                AgentResponseStatus::Queued | AgentResponseStatus::WaitingAction
            )
            || command.work.lease.is_some()
            || command.work.dispatch.is_some()
            || command.queue_expired(&crate::now_rfc3339())
        {
            return Ok(());
        }
        let current = fence
            .proof()
            .resources()
            .lineage
            .agent
            .clone()
            .ok_or(AgentOperationError::NotFound)?;
        let Some(turn_sequence) = current.conversation.active_turn else {
            return Ok(());
        };
        if !matches!(
            current.status,
            AgentStatus::Working | AgentStatus::WaitingApproval
        ) || current.requested_stop_reason.is_some()
            || current
                .conversation
                .operation
                .as_ref()
                .is_some_and(|operation| {
                    operation.intent == AgentOperationIntent::Interrupt
                        && operation.state != AgentOperationDelivery::PreWriteFailed
                })
        {
            return Ok(());
        }
        current
            .capabilities
            .require([
                RuntimeCapability::Conversation,
                RuntimeCapability::Interrupt,
            ])
            .map_err(|capability| AgentOperationError::CapabilityUnsupported { capability })?;
        self.require_first_message_dispatch_settled(&current)?;
        self.require_dispatch_available(&current)?;
        let (parent, queues) = self.parent_admission(&current).await?;
        if queues {
            return Ok(());
        }
        let service = Self {
            command_claim: None,
            execution_authority: None,
            channel_control_authority: None,
            channel_authority: Some(authority.with_supersession(ChannelSupersession {
                command: command.clone(),
                turn_sequence,
            })),
            trigger_authority: None,
            connections: self.connections.clone(),
            store: self.store.clone(),
            runtimes: self.runtimes.clone(),
        };
        let prepared = service.fence_interrupt(current, parent).await?;
        service.dispatch_prepared(prepared).await?;
        Ok(())
    }
}
