use layer3_adapter::{ContentBlock, ConversationInput};

use crate::{seconds_from_rfc3339, AgentId, RuntimeCapability};

use super::agent_operation_dispatch::{
    new_operation, next_sequence, operation_log, runtime_target, PreWriteRollback,
    PreparedAgentOperation,
};
use super::agent_operation_service::operation_store_error;
use super::queued_message::{
    queued_message_origin, QUEUED_MESSAGE_DISPATCH_GRACE_SECONDS, QUEUED_MESSAGE_RECOVERY_CODE,
    QUEUED_MESSAGE_RECOVERY_DETAIL, QUEUED_MESSAGE_RECOVERY_EVENT_CLASS,
};
use super::store::{
    AgentStore, SandboxLifecycleAgentMutation, SandboxLifecycleAgentScope,
    SandboxLifecycleExecutionStore, SandboxLifecycleMutation, SandboxMutationGuard,
};
use super::{
    Agent, AgentLogKind, AgentOperationError, AgentOperationFailure, AgentOperationIntent,
    AgentOperationReceipt, AgentOperationService, AgentStatus, QueuedMessageState, Sandbox,
    SandboxEventKind, SandboxWorkStep, TypedFailure,
};

impl AgentOperationService {
    /// Dispatch one exact FIFO queue item under a claimed sandbox lifecycle authority.
    pub async fn dispatch_lifecycle_queued_message(
        &self,
        sandbox: &Sandbox,
        agent_id: &AgentId,
        queued_message_sequence: u64,
        at: String,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        let current = AgentStore::get_agent(self.store.as_ref(), agent_id)
            .await
            .map_err(operation_store_error)?
            .ok_or(AgentOperationError::NotFound)?;
        let prepared = self
            .fence_lifecycle_queue_item(sandbox, current, queued_message_sequence, at)
            .await?;
        self.dispatch_prepared(prepared).await
    }

    /// Retire a dispatch fence whose runtime-write outcome outlived its grace period.
    ///
    /// This never retries the runtime input. Later runtime evidence or a late confirmed
    /// receipt may still promote the operation and queue item to a delivered state.
    pub async fn recover_stale_lifecycle_queue_dispatch(
        &self,
        sandbox: &Sandbox,
        agent_id: &AgentId,
        queued_message_sequence: u64,
        retry_at: String,
        at: String,
    ) -> Result<(), AgentOperationError> {
        let current = AgentStore::get_agent(self.store.as_ref(), agent_id)
            .await
            .map_err(operation_store_error)?
            .ok_or(AgentOperationError::NotFound)?;
        let queue_index =
            require_stale_queue_dispatch(sandbox, &current, queued_message_sequence, &at)?;
        let operation_sequence = current
            .conversation
            .operation
            .as_ref()
            .expect("the stale queue fence was validated")
            .sequence;
        let failure = AgentOperationFailure::new(
            QUEUED_MESSAGE_RECOVERY_CODE,
            QUEUED_MESSAGE_RECOVERY_DETAIL,
            at.clone(),
        )
        .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;

        let mut next_agent = current.clone();
        next_agent
            .conversation
            .operation
            .as_mut()
            .expect("the stale queue fence was validated")
            .require_recovery(failure)
            .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
        next_agent.conversation.queued_messages[queue_index]
            .require_recovery()
            .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
        let logs = vec![operation_log(&next_agent, operation_sequence)?];
        next_agent.revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| AgentOperationError::Corrupt("agent revision overflow".into()))?;
        next_agent.conversation.last_seq = current
            .conversation
            .last_seq
            .checked_add(1)
            .ok_or_else(|| AgentOperationError::Corrupt("agent log sequence overflow".into()))?;
        next_agent.updated_at = at.clone();

        let lease = sandbox
            .work
            .lease
            .as_ref()
            .ok_or(AgentOperationError::StaleWorkAuthority)?;
        let mut next_sandbox = sandbox.clone();
        next_sandbox
            .retry_lifecycle(retry_at, &lease.holder, lease.fence, at.clone())
            .map_err(|_| AgentOperationError::StaleWorkAuthority)?;
        SandboxLifecycleExecutionStore::commit_sandbox_lifecycle(
            self.store.as_ref(),
            SandboxLifecycleMutation {
                guard: SandboxMutationGuard::capture(sandbox),
                sandbox: next_sandbox,
                agents: vec![SandboxLifecycleAgentMutation {
                    expected_revision: current.revision,
                    agent: next_agent,
                    logs,
                }],
                agent_scope: SandboxLifecycleAgentScope::Touched,
                appended_at: at,
                events: vec![SandboxEventKind::WorkStepFailed {
                    step: SandboxWorkStep::RunLifecycleOperation,
                    failure: TypedFailure {
                        class: QUEUED_MESSAGE_RECOVERY_EVENT_CLASS.into(),
                        detail: QUEUED_MESSAGE_RECOVERY_DETAIL.into(),
                    },
                }],
            },
        )
        .await
        .map_err(operation_store_error)
    }

    async fn fence_lifecycle_queue_item(
        &self,
        sandbox: &Sandbox,
        current: Agent,
        queued_message_sequence: u64,
        at: String,
    ) -> Result<PreparedAgentOperation, AgentOperationError> {
        require_queue_dispatchable(sandbox, &current, queued_message_sequence)?;
        let target = runtime_target(&current)?;
        let operation_sequence = next_sequence(current.conversation.last_seq)?;
        let turn_sequence = operation_sequence
            .checked_add(1)
            .ok_or_else(|| AgentOperationError::Corrupt("agent turn sequence overflow".into()))?;
        let content = current
            .conversation
            .queued_messages
            .iter()
            .find(|message| message.sequence == queued_message_sequence)
            .expect("the exact queue item was validated")
            .content
            .clone();

        let mut next_agent = current.clone();
        let queued = next_agent
            .conversation
            .queued_messages
            .iter_mut()
            .find(|message| message.sequence == queued_message_sequence)
            .expect("the exact queue item was validated");
        queued
            .fence_dispatch(at.clone())
            .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
        next_agent.conversation.operation = Some(new_operation(
            operation_sequence,
            &target,
            turn_sequence,
            AgentOperationIntent::send_message(&content),
            &at,
        )?);
        next_agent.conversation.active_turn = Some(turn_sequence);
        next_agent
            .transition(AgentStatus::Working, at.clone(), None)
            .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
        let logs = vec![
            operation_log(&next_agent, operation_sequence)?,
            AgentLogKind::TurnStarted {
                model: Some(current.settings().model),
                turn_sequence,
                origin_key: Some(queued_message_origin(queued_message_sequence)),
            },
            AgentLogKind::StatusChanged {
                from: AgentStatus::Ready,
                to: AgentStatus::Working,
            },
        ];
        next_agent.conversation.last_seq = current
            .conversation
            .last_seq
            .checked_add(
                u64::try_from(logs.len()).map_err(|_| {
                    AgentOperationError::Corrupt("agent log sequence overflow".into())
                })?,
            )
            .ok_or_else(|| AgentOperationError::Corrupt("agent log sequence overflow".into()))?;

        let lease = sandbox
            .work
            .lease
            .as_ref()
            .ok_or(AgentOperationError::StaleWorkAuthority)?;
        let mut next_sandbox = sandbox.clone();
        next_sandbox
            .publish_lifecycle_queue_progress(&lease.holder, lease.fence, at.clone())
            .map_err(|_| AgentOperationError::StaleWorkAuthority)?;
        SandboxLifecycleExecutionStore::commit_sandbox_lifecycle(
            self.store.as_ref(),
            SandboxLifecycleMutation {
                guard: SandboxMutationGuard::capture(sandbox),
                sandbox: next_sandbox,
                agents: vec![SandboxLifecycleAgentMutation {
                    expected_revision: current.revision,
                    agent: next_agent,
                    logs,
                }],
                agent_scope: SandboxLifecycleAgentScope::Touched,
                appended_at: at,
                events: vec![
                    SandboxEventKind::AgentStatusChanged {
                        agent_id: current.id,
                        from: AgentStatus::Ready,
                        to: AgentStatus::Working,
                    },
                    SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::RunLifecycleOperation,
                    },
                ],
            },
        )
        .await
        .map_err(operation_store_error)?;

        let prompt_id = layer3_adapter::PromptIdentity::new(
            &target.conversation_id,
            target.runtime_generation,
            turn_sequence,
        );
        Ok(PreparedAgentOperation {
            target,
            operation_sequence,
            turn_sequence,
            required: vec![RuntimeCapability::Conversation],
            input: ConversationInput::RecoverablePrompt {
                content: vec![ContentBlock::from(content)],
                prompt_id,
            },
            rollback: PreWriteRollback::NewTurn,
            queued_message_sequence: Some(queued_message_sequence),
        })
    }
}

fn require_stale_queue_dispatch(
    sandbox: &Sandbox,
    agent: &Agent,
    queued_message_sequence: u64,
    at: &str,
) -> Result<usize, AgentOperationError> {
    if agent.sandbox_id != sandbox.id || agent.owner != sandbox.owner {
        return Err(AgentOperationError::NotFound);
    }
    let queue_index = agent
        .conversation
        .queued_messages
        .iter()
        .position(|message| message.sequence == queued_message_sequence)
        .ok_or_else(|| AgentOperationError::InvalidRequest("queued message is absent".into()))?;
    let queued = &agent.conversation.queued_messages[queue_index];
    let operation = agent
        .conversation
        .operation
        .as_ref()
        .ok_or(AgentOperationError::Conflict)?;
    let first_unsettled = agent
        .conversation
        .queued_messages
        .iter()
        .position(|message| message.state != QueuedMessageState::Delivered);
    let matching_intent = matches!(
        &operation.intent,
        AgentOperationIntent::SendMessage { payload_sha256 }
            if payload_sha256 == &queued.payload_sha256
    );
    let fenced_at = seconds_from_rfc3339(&operation.dispatch_fenced_at)
        .ok_or_else(|| AgentOperationError::Corrupt("operation fence time is invalid".into()))?;
    let observed_at = seconds_from_rfc3339(at)
        .ok_or_else(|| AgentOperationError::InvalidRequest("recovery time is invalid".into()))?;
    let stale = fenced_at
        .checked_add(QUEUED_MESSAGE_DISPATCH_GRACE_SECONDS)
        .is_some_and(|deadline| observed_at >= deadline);
    if agent.status != AgentStatus::Working
        || agent.conversation.active_turn != Some(operation.turn_sequence)
        || operation.state != super::AgentOperationDelivery::DispatchFenced
        || queued.state != QueuedMessageState::DispatchFenced
        || queued.dispatch_fenced_at.as_deref() != Some(operation.dispatch_fenced_at.as_str())
        || first_unsettled != Some(queue_index)
        || !matching_intent
        || !stale
    {
        return Err(AgentOperationError::Conflict);
    }
    Ok(queue_index)
}

fn require_queue_dispatchable(
    sandbox: &Sandbox,
    agent: &Agent,
    queued_message_sequence: u64,
) -> Result<(), AgentOperationError> {
    if agent.sandbox_id != sandbox.id || agent.owner != sandbox.owner {
        return Err(AgentOperationError::NotFound);
    }
    if agent.status != AgentStatus::Ready {
        return Err(AgentOperationError::StatusConflict {
            status: agent.status,
        });
    }
    if agent.conversation.active_turn.is_some()
        || agent.work.step.is_pending()
        || agent.work.lease.is_some()
        || agent.requested_stop_reason.is_some()
        || agent
            .conversation
            .operation
            .as_ref()
            .is_some_and(|operation| operation.blocks_dispatch())
    {
        return Err(AgentOperationError::Conflict);
    }
    let next = agent
        .conversation
        .queued_messages
        .iter()
        .find(|message| message.state != QueuedMessageState::Delivered)
        .ok_or_else(|| AgentOperationError::InvalidRequest("queued message is absent".into()))?;
    if next.sequence != queued_message_sequence || next.state != QueuedMessageState::Queued {
        return Err(AgentOperationError::Conflict);
    }
    Ok(())
}
