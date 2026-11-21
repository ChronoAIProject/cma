//! Legacy callers must honor the same exact-child closure as the command ledger.
use layer2_core::sandbox::{
    store::{AgentConversationMutation, SandboxStoreError},
    Agent, AgentLogKind, AgentOperationKind,
};
use mongodb::bson::Document;

pub(super) fn require(
    row: &Document,
    current: &Agent,
    mutation: &AgentConversationMutation,
) -> Result<(), SandboxStoreError> {
    if row.contains_key("channel_reuse_cleanup_execution") {
        return Err(SandboxStoreError::Conflict);
    }
    if !row.contains_key("channel_reuse_cleanup_admission") || mutation.command_claim.is_some() {
        // A claim is bound to the actual accepted ledger command later in the
        // same transaction. It is not accepted merely because it is present.
        return Ok(());
    }
    let previous = &current.conversation;
    let next = &mutation.agent.conversation;
    let new_operation = next.operation.as_ref().filter(|operation| {
        previous
            .operation
            .as_ref()
            .is_none_or(|old| old.sequence != operation.sequence)
    });
    let accepted_control = new_operation.is_some_and(|operation| {
        operation.intent.kind() != AgentOperationKind::SendMessage
            && previous.active_turn.is_some()
            && operation.turn_sequence == previous.active_turn.unwrap()
    });
    if new_operation
        .is_some_and(|operation| operation.intent.kind() == AgentOperationKind::SendMessage)
        || next.queued_messages.iter().any(|message| {
            !previous
                .queued_messages
                .iter()
                .any(|old| old.sequence == message.sequence)
        })
        || (next.first_message.is_some() && previous.first_message.is_none())
        || mutation.agent_logs.iter().any(|log| {
            matches!(log, AgentLogKind::TurnStarted { .. })
                || (matches!(log, AgentLogKind::OwnerMessage { .. }) && !accepted_control)
        })
    {
        return Err(SandboxStoreError::Conflict);
    }
    // Runtime observations and settlements remain legal. Previously accepted
    // legacy queued inputs use their existing lifecycle delivery transaction.
    Ok(())
}
