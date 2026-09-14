use super::*;

pub(super) fn validate_queue_prune(
    current: &Agent,
    change: &SandboxLifecycleAgentMutation,
) -> Result<(), SandboxStoreError> {
    let next = &change.agent;
    if current.status != AgentStatus::Ready
        || next.status != current.status
        || !change.logs.is_empty()
        || next.conversation.conversation_id != current.conversation.conversation_id
        || next.conversation.runtime_generation != current.conversation.runtime_generation
        || next.conversation.first_message != current.conversation.first_message
        || next.conversation.active_turn != current.conversation.active_turn
        || next.conversation.operation != current.conversation.operation
        || !next.conversation.queued_messages.is_empty()
    {
        return Err(invalid("queue completion pruned unrelated agent state"));
    }
    Ok(())
}

pub(super) fn latest_delivery_is_durable(agent: &Agent) -> bool {
    let Some(latest) = agent.conversation.queued_messages.last() else {
        return true;
    };
    agent
        .conversation
        .operation
        .as_ref()
        .is_some_and(|operation| {
            matches!(
                operation.state,
                AgentOperationDelivery::WriteConfirmed | AgentOperationDelivery::ExecutionObserved
            ) && matches!(
                &operation.intent,
                AgentOperationIntent::SendMessage { payload_sha256 }
                    if payload_sha256 == &latest.payload_sha256
            ) && latest.dispatch_fenced_at.as_deref() == Some(operation.dispatch_fenced_at.as_str())
        })
}
